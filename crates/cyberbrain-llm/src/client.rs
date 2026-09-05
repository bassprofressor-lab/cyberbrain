//! The HTTP client. Two routes, no vendor code: `POST {base}/chat/completions` and
//! `GET {base}/models`.
//!
//! A client can only exist after its endpoint passed [`crate::address::validate_endpoint`],
//! and it is pinned to the addresses that passed. Environment proxies are ignored and
//! redirects are refused: both would let bytes reach a host that was never validated.

use cyberbrain_core::{EgressGate, EgressPurpose, Error, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::address::{EndpointPolicy, Resolver, SystemResolver, ValidatedEndpoint};
use crate::audit::{AuditSink, CallKind, CallOutcome, InferenceEvent};
use crate::backend::{self, Backend, Probe};
use crate::types::{ChatRequest, ChatResponse, ModelInfo, Usage};
use crate::wire;

/// `[llm]` section of `cyberbrain.toml`.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// OpenAI-compatible base URL. Default `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    /// Model name sent in every request. Empty means "not chosen": calls degrade with a
    /// caveat until the operator picks one (`cyberbrain status` lists what the endpoint
    /// offers).
    pub model: String,
    /// Whole-request timeout for completions. Default 60 s: local models are slow to load.
    pub timeout: Duration,
    /// Timeout for `GET /models` and the status probe. Default 3 s.
    pub probe_timeout: Duration,
    /// Default `max_tokens`. Default 1024.
    pub max_tokens: u32,
    /// Default sampling temperature. Default 0.0: every use here wants determinism.
    pub temperature: f32,
    /// The operator's explicit waiver of the local-only rule. Default false.
    pub allow_public_endpoint: bool,
    /// PEM root certificate(s) to trust for an `https://` endpoint. Without this, no root
    /// store is compiled in and TLS to any host fails closed.
    pub tls_root_ca_pem: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: String::new(),
            timeout: Duration::from_secs(60),
            probe_timeout: Duration::from_secs(3),
            max_tokens: 1024,
            temperature: 0.0,
            allow_public_endpoint: false,
            tls_root_ca_pem: None,
        }
    }
}

impl LlmConfig {
    pub fn policy(&self) -> EndpointPolicy {
        EndpointPolicy {
            allow_public_endpoint: self.allow_public_endpoint,
            ..Default::default()
        }
    }
}

/// The one place in this crate that constructs an HTTP client.
///
/// It takes the [`EgressGate`] rather than a bare purpose, and that is the whole point:
/// holding a purpose only lets the call site *describe* itself, while holding a gate is the
/// only way to actually ask permission. The first version took a purpose, read as if it were
/// checked, and never called anything — every inference request left the machine without an
/// entry in the register. The signature now makes that shape impossible to write.
///
/// Permission is asked again before each request (see [`LlmClient::authorise`]); this call
/// authorises the channel, not the traffic.
pub fn build_http_client(
    gate: &dyn EgressGate,
    purpose: EgressPurpose,
    endpoint: &ValidatedEndpoint,
    cfg: &LlmConfig,
) -> Result<reqwest::Client> {
    if purpose != EgressPurpose::LocalInference {
        return Err(Error::PolicyRefusal {
            profile: crate::address::POLICY_NAME.into(),
            reason: format!(
                "cyberbrain-llm only performs {:?} egress",
                EgressPurpose::LocalInference
            ),
        });
    }

    gate.permit(purpose, &endpoint.summary())?;

    // reqwest with `rustls-no-provider` panics at build time without a provider. Ring was
    // chosen for the tree (no cmake, no aws-lc-sys); installing twice is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut b = reqwest::Client::builder()
        // Never route through HTTP(S)_PROXY from the environment: a proxy is a host we
        // did not validate.
        .no_proxy()
        // Never follow a redirect: the target would be a host we did not validate.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(cfg.timeout)
        .connect_timeout(Duration::from_secs(5))
        .user_agent(concat!("cyberbrain/", env!("CARGO_PKG_VERSION")));

    // Pin the validated addresses. For a hostname this bypasses DNS at connect time, so
    // a changed answer (rebinding) cannot move the connection.
    if !endpoint.is_literal() {
        b = b.resolve_to_addrs(&endpoint.host, &endpoint.addrs);
    }

    if let Some(pem) = &cfg.tls_root_ca_pem {
        for cert in reqwest::Certificate::from_pem_bundle(pem.as_bytes())
            .map_err(|e| Error::Config(format!("llm.tls_root_ca_pem: {e}")))?
        {
            b = b.add_root_certificate(cert);
        }
    }

    b.build()
        .map_err(|e| Error::Llm(format!("could not build HTTP client: {e}")))
}

/// A validated, pinned, audited connection to one local inference endpoint.
#[derive(Clone)]
pub struct LlmClient {
    cfg: LlmConfig,
    endpoint: ValidatedEndpoint,
    http: reqwest::Client,
    audit: Arc<dyn AuditSink>,
    gate: Arc<dyn EgressGate>,
}

impl std::fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmClient")
            .field("endpoint", &self.endpoint.summary())
            .field("model", &self.cfg.model)
            .finish()
    }
}

impl LlmClient {
    /// Validate the configured endpoint with the system resolver and build a client.
    /// A refusal is `Error::PolicyRefusal` and is audited before being returned.
    pub async fn connect(
        cfg: LlmConfig,
        audit: Arc<dyn AuditSink>,
        gate: Arc<dyn EgressGate>,
    ) -> Result<Self> {
        Self::connect_with_resolver(cfg, audit, gate, &SystemResolver).await
    }

    /// Ask the register before every request, not once per client.
    ///
    /// SPEC §12.1 says all outbound I/O passes the wrapper. A single check at construction
    /// would authorise a channel and then let an unbounded number of requests ride it
    /// unrecorded, which is exactly the accounting the register exists to provide.
    fn authorise(&self) -> Result<()> {
        self.gate
            .permit(EgressPurpose::LocalInference, &self.endpoint.summary())
    }

    /// As [`connect`](Self::connect), with an injected resolver.
    pub async fn connect_with_resolver(
        cfg: LlmConfig,
        audit: Arc<dyn AuditSink>,
        gate: Arc<dyn EgressGate>,
        resolver: &dyn Resolver,
    ) -> Result<Self> {
        let started = Instant::now();
        let endpoint =
            match crate::address::validate_endpoint(&cfg.base_url, &cfg.policy(), resolver).await {
                Ok(v) => v,
                Err(e) => {
                    audit.record_inference(InferenceEvent {
                        at: jiff::Timestamp::now(),
                        purpose: EgressPurpose::LocalInference,
                        call: CallKind::EndpointValidation,
                        task: None,
                        endpoint: cfg.base_url.clone(),
                        resolved: Vec::new(),
                        public_waived: false,
                        model: cfg.model.clone(),
                        outcome: match &e {
                            Error::PolicyRefusal { reason, .. } => {
                                CallOutcome::Refused(reason.clone())
                            }
                            other => CallOutcome::Unreachable(other.to_string()),
                        },
                        usage: None,
                        elapsed: started.elapsed(),
                    });
                    return Err(e);
                }
            };
        let http = build_http_client(gate.as_ref(), EgressPurpose::LocalInference, &endpoint, &cfg)?;
        audit.record_inference(InferenceEvent {
            at: jiff::Timestamp::now(),
            purpose: EgressPurpose::LocalInference,
            call: CallKind::EndpointValidation,
            task: None,
            endpoint: endpoint.base_url.to_string(),
            resolved: endpoint.addrs.clone(),
            public_waived: endpoint.public_waived,
            model: cfg.model.clone(),
            outcome: CallOutcome::Ok,
            usage: None,
            elapsed: started.elapsed(),
        });
        Ok(Self {
            cfg,
            endpoint,
            http,
            audit,
            gate,
        })
    }

    pub fn config(&self) -> &LlmConfig {
        &self.cfg
    }

    pub fn endpoint(&self) -> &ValidatedEndpoint {
        &self.endpoint
    }

    /// Caveat to attach whenever this client is used with a waived public endpoint.
    /// `None` for a local one.
    pub fn waiver_caveat(&self) -> Option<String> {
        self.endpoint.public_waived.then(|| {
            format!(
                "note text is sent to a non-local endpoint ({}) because allow_public_endpoint \
                 is set",
                self.endpoint.summary()
            )
        })
    }

    fn url(&self, route: &str) -> String {
        format!(
            "{}/{}",
            self.endpoint.base_url.as_str().trim_end_matches('/'),
            route
        )
    }

    #[allow(clippy::too_many_arguments)] // one audit row, every field named at the call site
    fn record(
        &self,
        call: CallKind,
        task: Option<&'static str>,
        url: &str,
        model: &str,
        outcome: CallOutcome,
        usage: Option<Usage>,
        started: Instant,
    ) {
        self.audit.record_inference(InferenceEvent {
            at: jiff::Timestamp::now(),
            purpose: EgressPurpose::LocalInference,
            call,
            task,
            endpoint: url.to_string(),
            resolved: self.endpoint.addrs.clone(),
            public_waived: self.endpoint.public_waived,
            model: model.to_string(),
            outcome,
            usage,
            elapsed: started.elapsed(),
        });
    }

    fn model_for(&self, req: &ChatRequest) -> Result<String> {
        let m = req.model.as_deref().unwrap_or(&self.cfg.model).trim();
        if m.is_empty() {
            return Err(Error::Config(
                "llm.model is empty; pick one of the models `cyberbrain status` lists".into(),
            ));
        }
        Ok(m.to_string())
    }

    /// `POST /chat/completions`, non-streaming.
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let model = self.model_for(req)?;
        let url = self.url("chat/completions");
        let started = Instant::now();
        let body = wire::ChatCompletionRequest {
            model: &model,
            messages: &req.messages,
            max_tokens: Some(req.max_tokens.unwrap_or(self.cfg.max_tokens)),
            temperature: Some(req.temperature.unwrap_or(self.cfg.temperature)),
            stream: false,
            stream_options: None,
        };

        self.authorise()?;
        let resp = self.http.post(&url).json(&body).send().await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(
                    CallKind::ChatCompletion,
                    req.task,
                    &url,
                    &model,
                    outcome,
                    None,
                    started,
                );
                return Err(err);
            }
        };
        let status = resp.status();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(
                    CallKind::ChatCompletion,
                    req.task,
                    &url,
                    &model,
                    outcome,
                    None,
                    started,
                );
                return Err(err);
            }
        };
        if !status.is_success() {
            let err = Error::Llm(format!(
                "{url} answered {}: {}",
                status.as_u16(),
                server_message(&text)
            ));
            self.record(
                CallKind::ChatCompletion,
                req.task,
                &url,
                &model,
                CallOutcome::BadStatus(status.as_u16()),
                None,
                started,
            );
            return Err(err);
        }

        let parsed: wire::ChatCompletionResponse = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("{url} returned a body that is not a chat completion: {e}");
                self.record(
                    CallKind::ChatCompletion,
                    req.task,
                    &url,
                    &model,
                    CallOutcome::BadResponse(msg.clone()),
                    None,
                    started,
                );
                return Err(Error::Llm(msg));
            }
        };
        let usage = parsed.usage.map(Usage::from);
        let first = parsed.choices.into_iter().next();
        let (content, finish_reason) = match first {
            Some(c) => (
                c.message.and_then(|m| m.content).unwrap_or_default(),
                c.finish_reason,
            ),
            None => {
                let msg = format!("{url} returned no choices");
                self.record(
                    CallKind::ChatCompletion,
                    req.task,
                    &url,
                    &model,
                    CallOutcome::BadResponse(msg.clone()),
                    usage,
                    started,
                );
                return Err(Error::Llm(msg));
            }
        };
        self.record(
            CallKind::ChatCompletion,
            req.task,
            &url,
            &model,
            CallOutcome::Ok,
            usage,
            started,
        );
        Ok(ChatResponse {
            content,
            model: parsed.model,
            finish_reason,
            usage,
        })
    }

    /// **EXPERIMENTAL.** `POST /chat/completions` with `stream: true`. Each text delta is
    /// handed to `on_delta` as it arrives; the assembled response is returned at the end.
    /// Token usage is only present if the server sends a usage chunk (OpenAI needs
    /// `stream_options.include_usage`, which we request; Ollama sends it regardless;
    /// others may not). The audit row records whatever was received.
    ///
    /// Not used by any spec feature yet; the four features are non-streaming because their
    /// output is parsed as a whole. The API may change.
    pub async fn chat_streaming(
        &self,
        req: &ChatRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatResponse> {
        let model = self.model_for(req)?;
        let url = self.url("chat/completions");
        let started = Instant::now();
        let body = wire::ChatCompletionRequest {
            model: &model,
            messages: &req.messages,
            max_tokens: Some(req.max_tokens.unwrap_or(self.cfg.max_tokens)),
            temperature: Some(req.temperature.unwrap_or(self.cfg.temperature)),
            stream: true,
            stream_options: Some(wire::StreamOptions {
                include_usage: true,
            }),
        };
        let kind = CallKind::ChatCompletionStream;

        self.authorise()?;
        let mut resp = match self.http.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(kind, req.task, &url, &model, outcome, None, started);
                return Err(err);
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            self.record(
                kind,
                req.task,
                &url,
                &model,
                CallOutcome::BadStatus(status.as_u16()),
                None,
                started,
            );
            return Err(Error::Llm(format!(
                "{url} answered {}: {}",
                status.as_u16(),
                server_message(&text)
            )));
        }

        let mut content = String::new();
        let mut finish_reason = None;
        let mut reported_model = None;
        let mut usage: Option<Usage> = None;
        let mut buf: Vec<u8> = Vec::new();
        let mut done = false;

        loop {
            let chunk = match resp.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    let (outcome, err) = transport_failure(&url, &e);
                    self.record(kind, req.task, &url, &model, outcome, usage, started);
                    return Err(err);
                }
            };
            buf.extend_from_slice(&chunk);
            // SSE events are separated by a blank line. Process every complete event.
            while let Some(pos) = find_event_end(&buf) {
                let event: Vec<u8> = buf.drain(..pos.0).collect();
                buf.drain(..pos.1);
                let event = String::from_utf8_lossy(&event);
                for line in event.lines() {
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        done = true;
                        continue;
                    }
                    match serde_json::from_str::<wire::ChatCompletionChunk>(data) {
                        Ok(c) => {
                            if c.model.is_some() {
                                reported_model = c.model;
                            }
                            if let Some(u) = c.usage {
                                usage = Some(u.into());
                            }
                            for choice in c.choices {
                                if let Some(text) = choice.delta.and_then(|d| d.content)
                                    && !text.is_empty()
                                {
                                    on_delta(&text);
                                    content.push_str(&text);
                                }
                                if choice.finish_reason.is_some() {
                                    finish_reason = choice.finish_reason;
                                }
                            }
                        }
                        Err(e) => {
                            let msg =
                                format!("{url} streamed a chunk that is not a completion: {e}");
                            self.record(
                                kind,
                                req.task,
                                &url,
                                &model,
                                CallOutcome::BadResponse(msg.clone()),
                                usage,
                                started,
                            );
                            return Err(Error::Llm(msg));
                        }
                    }
                }
            }
            if done {
                break;
            }
        }

        self.record(
            kind,
            req.task,
            &url,
            &model,
            CallOutcome::Ok,
            usage,
            started,
        );
        Ok(ChatResponse {
            content,
            model: reported_model,
            finish_reason,
            usage,
        })
    }

    /// `GET /models`. Returns the list plus the response headers, which the backend
    /// identifier reads.
    async fn models_with_headers(&self) -> Result<(Vec<ModelInfo>, Vec<(String, String)>)> {
        let url = self.url("models");
        let started = Instant::now();
        self.authorise()?;
        let resp = self
            .http
            .get(&url)
            .timeout(self.cfg.probe_timeout)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(CallKind::ListModels, None, &url, "", outcome, None, started);
                return Err(err);
            }
        };
        let status = resp.status();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.record(
                CallKind::ListModels,
                None,
                &url,
                "",
                CallOutcome::BadStatus(status.as_u16()),
                None,
                started,
            );
            return Err(Error::Llm(format!(
                "{url} answered {}: {}",
                status.as_u16(),
                server_message(&text)
            )));
        }
        let parsed: wire::ModelsResponse = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("{url} returned a body that is not a model list: {e}");
                self.record(
                    CallKind::ListModels,
                    None,
                    &url,
                    "",
                    CallOutcome::BadResponse(msg.clone()),
                    None,
                    started,
                );
                return Err(Error::Llm(msg));
            }
        };
        self.record(
            CallKind::ListModels,
            None,
            &url,
            "",
            CallOutcome::Ok,
            None,
            started,
        );
        let models = parsed
            .data
            .into_iter()
            .map(|m| ModelInfo {
                id: m.id,
                owned_by: m.owned_by,
            })
            .collect();
        Ok((models, headers))
    }

    /// `GET /models`.
    pub async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self.models_with_headers().await?.0)
    }

    /// Ask the endpoint what it is, for the status screen. Never fails: an unreachable
    /// endpoint is a `Probe` with `reachable = false` and a caveat.
    pub async fn probe(&self) -> Probe {
        let started = Instant::now();
        match self.models_with_headers().await {
            Ok((models, headers)) => {
                let (backend, evidence) = backend::identify(&headers, &models);
                let configured_model_listed = models.iter().any(|m| m.id == self.cfg.model);
                let mut caveat = None;
                if !self.cfg.model.is_empty() && !configured_model_listed {
                    caveat = Some(format!(
                        "configured model {:?} is not among the {} models the endpoint lists",
                        self.cfg.model,
                        models.len()
                    ));
                }
                Probe {
                    reachable: true,
                    backend,
                    endpoint: self.endpoint.summary(),
                    models: models.into_iter().map(|m| m.id).collect(),
                    configured_model: self.cfg.model.clone(),
                    configured_model_listed,
                    evidence,
                    caveat,
                    latency: started.elapsed(),
                }
            }
            Err(e) => Probe {
                reachable: false,
                backend: Backend::Unknown,
                endpoint: self.endpoint.summary(),
                models: Vec::new(),
                configured_model: self.cfg.model.clone(),
                configured_model_listed: false,
                evidence: Vec::new(),
                caveat: Some(e.to_string()),
                latency: started.elapsed(),
            },
        }
    }
}

/// Position of the next SSE event terminator in `buf`: (event end, terminator length).
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| (p, 4));
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| (p, 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// Pull a readable message out of an error body, if the server sent one.
fn server_message(body: &str) -> String {
    if let Ok(env) = serde_json::from_str::<wire::ErrorEnvelope>(body) {
        return env.error.message().to_string();
    }
    let t = body.trim();
    if t.is_empty() {
        "(empty body)".to_string()
    } else {
        t.chars().take(200).collect()
    }
}

/// Map a reqwest transport error to an audit outcome and a user-facing error.
fn transport_failure(url: &str, e: &reqwest::Error) -> (CallOutcome, Error) {
    if e.is_timeout() {
        (
            CallOutcome::TimedOut,
            Error::Llm(format!(
                "{url} did not answer within the configured timeout"
            )),
        )
    } else if e.is_redirect() {
        let msg = format!("{url} tried to redirect; redirects are refused");
        (CallOutcome::Unreachable(msg.clone()), Error::Llm(msg))
    } else {
        // reqwest wraps the io error; the outer Display is "error sending request",
        // so walk to the innermost source for something useful.
        let mut msg = e.to_string();
        let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
        while let Some(s) = src {
            msg = s.to_string();
            src = s.source();
        }
        let msg = format!("{url} unreachable: {msg}");
        (CallOutcome::Unreachable(msg.clone()), Error::Llm(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemoryAuditSink;
    use crate::mock::{MockResponse, MockServer};
    use crate::types::ChatMessage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A gate that permits and counts. The real one lives in `cyberbrain-policy`, which
    /// this crate must not depend on.
    #[derive(Debug, Default)]
    struct CountingGate(AtomicUsize);

    impl EgressGate for CountingGate {
        fn permit(&self, _p: EgressPurpose, _d: &str) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A gate that refuses everything, standing in for a profile that forbids the call.
    #[derive(Debug)]
    struct ClosedGate;

    impl EgressGate for ClosedGate {
        fn permit(&self, _p: EgressPurpose, d: &str) -> Result<()> {
            Err(Error::PolicyRefusal {
                profile: "test".into(),
                reason: format!("{d} refused by test gate"),
            })
        }
    }

    fn open_gate() -> Arc<dyn EgressGate> {
        Arc::new(CountingGate::default())
    }

    fn cfg(base: &str) -> LlmConfig {
        LlmConfig {
            base_url: base.to_string(),
            model: "test-model".into(),
            timeout: Duration::from_secs(5),
            probe_timeout: Duration::from_secs(2),
            ..Default::default()
        }
    }

    fn completion_json(text: &str) -> String {
        serde_json::json!({
            "id": "x", "object": "chat.completion", "model": "test-model:latest",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 7, "total_tokens": 19}
        })
        .to_string()
    }

    #[tokio::test]
    async fn chat_round_trip_records_endpoint_model_and_tokens() {
        let server = MockServer::start(|req| {
            assert_eq!(req.method, "POST");
            assert_eq!(req.path, "/v1/chat/completions");
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["model"], "test-model");
            assert_eq!(body["stream"], false);
            assert_eq!(body["messages"][0]["role"], "user");
            MockResponse::json(200, &completion_json("hello"))
        })
        .await;
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), audit.clone(), open_gate())
            .await
            .unwrap();

        let out = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]).with_task("test"))
            .await
            .unwrap();
        assert_eq!(out.content, "hello");
        assert_eq!(out.model.as_deref(), Some("test-model:latest"));
        assert_eq!(out.usage.unwrap().total_tokens, Some(19));

        let rows = audit.rows();
        assert_eq!(rows.len(), 2, "validation row + call row");
        assert_eq!(rows[0].call, CallKind::EndpointValidation);
        let row = &rows[1];
        assert_eq!(row.call, CallKind::ChatCompletion);
        assert_eq!(row.task, Some("test"));
        assert_eq!(row.model, "test-model");
        assert!(
            row.endpoint.ends_with("/v1/chat/completions"),
            "{}",
            row.endpoint
        );
        assert_eq!(row.resolved, vec![server.addr]);
        assert_eq!(row.usage.unwrap().prompt_tokens, Some(12));
        assert!(row.outcome.is_ok());
        assert!(!row.public_waived);
    }

    #[tokio::test]
    async fn missing_usage_is_none_not_zero() {
        let server = MockServer::start(|_| {
            MockResponse::json(
                200,
                r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
            )
        })
        .await;
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), audit.clone(), open_gate())
            .await
            .unwrap();
        let out = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap();
        assert_eq!(out.usage, None);
        assert_eq!(audit.rows()[1].usage, None);
    }

    #[tokio::test]
    async fn empty_model_degrades_before_any_request() {
        let server = MockServer::start(|_| panic!("no request may be sent")).await;
        let mut c = cfg(&server.base_url("/v1"));
        c.model.clear();
        let client = LlmClient::connect(c, MemoryAuditSink::new(), open_gate()).await.unwrap();
        let err = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err}");
    }

    #[tokio::test]
    async fn server_error_is_reported_with_its_message() {
        let server = MockServer::start(|_| {
            MockResponse::json(
                404,
                r#"{"error":{"message":"model 'test-model' not found","type":"api_error"}}"#,
            )
        })
        .await;
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), audit.clone(), open_gate())
            .await
            .unwrap();
        let err = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
        assert!(err.to_string().contains("not found"), "{err}");
        assert_eq!(audit.rows()[1].outcome, CallOutcome::BadStatus(404));
    }

    #[tokio::test]
    async fn garbage_body_is_a_bad_response() {
        let server = MockServer::start(|_| MockResponse::json(200, "<html>not json</html>")).await;
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), audit.clone(), open_gate())
            .await
            .unwrap();
        let err = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Llm(_)));
        assert!(matches!(
            audit.rows()[1].outcome,
            CallOutcome::BadResponse(_)
        ));
    }

    #[tokio::test]
    async fn unreachable_endpoint_is_an_llm_error_not_a_panic() {
        // Bind then drop: the port is free and nothing listens.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&format!("http://127.0.0.1:{port}/v1")), audit.clone(), open_gate())
            .await
            .unwrap();
        let err = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Llm(_)), "{err}");
        assert!(err.to_string().contains("unreachable"), "{err}");
        assert!(matches!(
            audit.rows()[1].outcome,
            CallOutcome::Unreachable(_)
        ));
    }

    #[tokio::test]
    async fn slow_endpoint_times_out_within_budget() {
        let server = MockServer::start(|_| {
            MockResponse::json(200, &completion_json("late")).delayed(Duration::from_secs(3))
        })
        .await;
        let mut c = cfg(&server.base_url("/v1"));
        c.timeout = Duration::from_millis(300);
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(c, audit.clone(), open_gate()).await.unwrap();
        let started = Instant::now();
        let err = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(err.to_string().contains("timeout"), "{err}");
        assert_eq!(audit.rows()[1].outcome, CallOutcome::TimedOut);
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let server = MockServer::start(|_| {
            MockResponse::json(307, "").header("location", "http://203.0.113.7/v1/chat/completions")
        })
        .await;
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), audit.clone(), open_gate())
            .await
            .unwrap();
        let err = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("307"), "{err}");
        // Only the configured endpoint ever appears in the audit trail.
        for row in audit.rows() {
            assert!(
                row.endpoint.starts_with(&server.base_url("")),
                "{}",
                row.endpoint
            );
            assert_eq!(row.resolved, vec![server.addr]);
        }
    }

    #[tokio::test]
    async fn refused_endpoint_never_builds_a_client_and_is_audited() {
        let audit = MemoryAuditSink::new();
        let err = LlmClient::connect(cfg("http://203.0.113.7:11434/v1"), audit.clone(), open_gate())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { .. }), "{err}");
        assert_eq!(err.exit_code(), 3);
        let rows = audit.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].call, CallKind::EndpointValidation);
        assert!(matches!(rows[0].outcome, CallOutcome::Refused(_)));
        assert!(rows[0].resolved.is_empty());
    }

    #[tokio::test]
    async fn hostname_is_pinned_to_validated_addresses() {
        // "inference.local" does not exist in any DNS. If the request reaches the mock, the
        // client connected to the address the validator returned, not to a DNS lookup.
        use crate::address::Resolver;
        use std::collections::HashMap;
        struct Static(HashMap<String, Vec<std::net::IpAddr>>);
        impl Resolver for Static {
            fn resolve<'a>(
                &'a self,
                host: &'a str,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = std::io::Result<Vec<std::net::IpAddr>>>
                        + Send
                        + 'a,
                >,
            > {
                let a = self.0.get(host).cloned().unwrap_or_default();
                Box::pin(async move { Ok(a) })
            }
        }
        let server = MockServer::start(|req| {
            // The Host header carries the configured name; the TCP connection carries the
            // pinned address. Both must hold.
            assert!(
                req.headers
                    .get("host")
                    .is_some_and(|h| h.starts_with("inference.local:")),
                "host header was {:?}",
                req.headers.get("host")
            );
            MockResponse::json(200, &completion_json("pinned"))
        })
        .await;
        let mut table = HashMap::new();
        table.insert("inference.local".to_string(), vec![server.addr.ip()]);
        let c = cfg(&format!("http://inference.local:{}/v1", server.addr.port()));
        let client = LlmClient::connect_with_resolver(c, MemoryAuditSink::new(), open_gate(), &Static(table))
            .await
            .unwrap();
        assert_eq!(client.endpoint().host, "inference.local");
        let out = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await
            .unwrap();
        assert_eq!(out.content, "pinned");
    }

    #[tokio::test]
    async fn environment_proxy_is_ignored() {
        // A proxy that nothing should ever reach.
        let proxy =
            MockServer::start(|_| panic!("request went through the environment proxy")).await;
        // SAFETY: tests in this crate run under a multi-thread runtime; env mutation is
        // process-wide. This test sets and unsets a variable nobody else reads.
        unsafe {
            std::env::set_var("HTTP_PROXY", format!("http://{}", proxy.addr));
            std::env::set_var("http_proxy", format!("http://{}", proxy.addr));
        }
        let server =
            MockServer::start(|_| MockResponse::json(200, &completion_json("direct"))).await;
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), MemoryAuditSink::new(), open_gate())
            .await
            .unwrap();
        let out = client
            .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
            .await;
        unsafe {
            std::env::remove_var("HTTP_PROXY");
            std::env::remove_var("http_proxy");
        }
        assert_eq!(out.unwrap().content, "direct");
    }

    #[tokio::test]
    async fn models_and_probe() {
        let server = MockServer::start(|req| {
            assert_eq!(req.path, "/v1/models");
            MockResponse::json(
                200,
                r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"library"},{"id":"other","object":"model","owned_by":"library"}]}"#,
            )
        })
        .await;
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), MemoryAuditSink::new(), open_gate())
            .await
            .unwrap();
        let models = client.models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].owned_by.as_deref(), Some("library"));

        let probe = client.probe().await;
        assert!(probe.reachable);
        assert_eq!(probe.backend, Backend::Ollama);
        assert!(probe.configured_model_listed);
        assert_eq!(probe.caveat, None);
    }

    /// The register only means anything if a refusal actually stops the bytes. Written
    /// against the broken state first: before `authorise()` existed the client held a
    /// purpose and never asked anything, so the request reached the server and the mock's
    /// panic fired.
    #[tokio::test]
    async fn a_refusing_gate_stops_the_request_before_it_is_sent() {
        let server = MockServer::start(|_| panic!("a refused call must never reach the endpoint"))
            .await;
        let err = LlmClient::connect(
            cfg(&server.base_url("/v1")),
            MemoryAuditSink::new(),
            Arc::new(ClosedGate),
        )
        .await
        .expect_err("a closed gate must not yield a client");
        assert!(matches!(err, Error::PolicyRefusal { .. }), "{err:?}");
    }

    /// Permission is asked per request, not once per client. A single check at construction
    /// would authorise a channel and then let unlimited traffic ride it unrecorded.
    #[tokio::test]
    async fn every_request_asks_the_gate_again() {
        let server = MockServer::start(|_| {
            MockResponse::json(
                200,
                r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
            )
        })
        .await;
        let gate = Arc::new(CountingGate::default());
        let client = LlmClient::connect(
            cfg(&server.base_url("/v1")),
            MemoryAuditSink::new(),
            gate.clone(),
        )
        .await
        .unwrap();

        let after_connect = gate.0.load(Ordering::SeqCst);
        assert_eq!(after_connect, 1, "the channel itself is authorised once");

        for _ in 0..3 {
            client
                .chat(&ChatRequest::new(vec![ChatMessage::user("hi")]))
                .await
                .unwrap();
        }
        assert_eq!(
            gate.0.load(Ordering::SeqCst),
            after_connect + 3,
            "each request must ask again"
        );
    }

    #[tokio::test]
    async fn probe_of_dead_endpoint_never_fails() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let client = LlmClient::connect(
            cfg(&format!("http://127.0.0.1:{port}/v1")),
            MemoryAuditSink::new(),
        open_gate(),
        )
        .await
        .unwrap();
        let probe = client.probe().await;
        assert!(!probe.reachable);
        assert_eq!(probe.backend, Backend::Unknown);
        assert!(probe.caveat.is_some());
    }

    #[tokio::test]
    async fn probe_names_a_model_the_endpoint_does_not_have() {
        let server = MockServer::start(|_| {
            MockResponse::json(
                200,
                r#"{"data":[{"id":"something-else","owned_by":"library"}]}"#,
            )
        })
        .await;
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), MemoryAuditSink::new(), open_gate())
            .await
            .unwrap();
        let probe = client.probe().await;
        assert!(probe.reachable);
        assert!(!probe.configured_model_listed);
        assert!(probe.caveat.unwrap().contains("test-model"));
    }

    #[tokio::test]
    async fn streaming_assembles_deltas_and_usage() {
        let sse = concat!(
            "data: {\"model\":\"m\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let server = MockServer::start(move |req| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["stream"], true);
            assert_eq!(body["stream_options"]["include_usage"], true);
            MockResponse::new(200, sse.as_bytes().to_vec())
                .header("content-type", "text/event-stream")
        })
        .await;
        let audit = MemoryAuditSink::new();
        let client = LlmClient::connect(cfg(&server.base_url("/v1")), audit.clone(), open_gate())
            .await
            .unwrap();
        let mut deltas = Vec::new();
        let out = client
            .chat_streaming(&ChatRequest::new(vec![ChatMessage::user("hi")]), &mut |d| {
                deltas.push(d.to_string())
            })
            .await
            .unwrap();
        assert_eq!(deltas, vec!["hel", "lo"]);
        assert_eq!(out.content, "hello");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
        assert_eq!(out.usage.unwrap().total_tokens, Some(5));
        assert_eq!(audit.rows()[1].call, CallKind::ChatCompletionStream);
        assert_eq!(audit.rows()[1].usage.unwrap().total_tokens, Some(5));
    }

    #[test]
    fn build_refuses_any_other_purpose() {
        let ep = ValidatedEndpoint {
            base_url: reqwest::Url::parse("http://127.0.0.1:1/v1").unwrap(),
            host: "127.0.0.1".into(),
            port: 1,
            addrs: vec!["127.0.0.1:1".parse().unwrap()],
            classes: vec![crate::address::AddressClass::Loopback],
            public_waived: false,
        };
        let err = build_http_client(&CountingGate::default(), EgressPurpose::ModelDownload, &ep, &LlmConfig::default())
            .unwrap_err();
        assert!(matches!(err, Error::PolicyRefusal { .. }));
    }

    #[test]
    fn sse_event_boundary_detection() {
        assert_eq!(find_event_end(b"data: a\n\ndata: b"), Some((7, 2)));
        assert_eq!(find_event_end(b"data: a\r\n\r\nrest"), Some((7, 4)));
        assert_eq!(find_event_end(b"data: partial"), None);
    }
}
