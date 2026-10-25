//! Embeddings from a local Ollama, as the alternative to the in-process model2vec backend
//! (SPEC §6: "an external service").
//!
//! # Why this exists
//!
//! Static token embeddings have no context and no word order. On this project's own notes
//! (2026-10-08, 34 questions, dense only) potion-multilingual-128M put the right note in the
//! top three for 65 % of the questions; embeddinggemma:300m-qat-q4_0 behind Ollama did so for
//! 94 %. The transformer costs a forward pass per text, which is why it is a choice and not
//! the default.
//!
//! # What it keeps from the rest of this crate
//!
//! - The address rules: the base URL is validated like the inference endpoint, the client is
//!   pinned to the addresses that passed, no proxy, no redirects.
//! - The register: every request asks the [`EgressGate`] first, as `LocalInference`.
//! - The audit: one [`InferenceEvent`] per request, refusals and failures included.
//!
//! # What it adds
//!
//! - **The weights digest is the manifest.** The vectors are only comparable while the weights
//!   behind the tag stay the same, and `ollama pull` can move a tag. Before embedding, and
//!   again at most every [`VERIFY_EVERY`], the weights blob Ollama loads for the model (the
//!   `FROM …/sha256-<digest>` line of `POST /api/show`) is compared with the digest in the
//!   configuration, and the model's own embedding length with `dim`. A difference is refused,
//!   never mixed into the index. A failed check is remembered for as long, so a scan over a
//!   thousand notes asks once and not a thousand times.
//!
//!   Not the digest `GET /api/tags` lists: Ollama 0.40 lists the same tag twice there (its
//!   v2 manifest and a llama.cpp conversion, measured 2026-10-08), with two digests, neither of
//!   them the weights. The blob behind `FROM` is content-addressed and is what the runner maps.
//! - **Degenerate input** (empty or whitespace only) yields the all-zero vector without a
//!   request (SPEC §6.3). Ollama would return a vector for it, and a vector for nothing is
//!   noise that ranks.
//! - **A down endpoint is remembered** for [`BACKOFF`]: a scan over a thousand notes with
//!   Ollama stopped would otherwise try, fail and audit a thousand times.
//!
//! # Sync over async
//!
//! [`Embedder`] is synchronous and is called both from plain threads and from inside a tokio
//! runtime (the MCP server). Starting a runtime inside a runtime panics, and a reqwest client
//! must stay on the runtime its connections were made on. So the embedder owns one worker
//! thread with its own current-thread runtime and the HTTP client, and hands it jobs over a
//! channel. The thread ends when the embedder is dropped.

use cyberbrain_core::{EgressGate, EgressPurpose, Embedder, Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::address::{EndpointPolicy, SystemResolver, ValidatedEndpoint};
use crate::audit::{AuditSink, CallKind, CallOutcome, InferenceEvent};
use crate::client::{LlmConfig, build_http_client, transport_failure};
use crate::types::Usage;

/// How long a verified digest is trusted before it is asked again.
pub const VERIFY_EVERY: Duration = Duration::from_secs(30);

/// How long an unreachable endpoint is not asked again.
pub const BACKOFF: Duration = Duration::from_secs(30);

/// Everything the embedder needs, already taken out of `cyberbrain.toml`.
#[derive(Debug, Clone)]
pub struct OllamaEmbedConfig {
    /// Ollama's base URL, without `/v1`.
    pub base_url: String,
    pub model: String,
    /// The weights blob, 64 hex digits without a `sha256:` prefix (`ollama show --modelfile`).
    pub digest: String,
    pub dim: usize,
    pub query_prefix: String,
    pub document_prefix: String,
    pub num_ctx: u32,
    pub batch: usize,
    pub timeout: Duration,
    pub keep_alive: String,
    pub allow_public_endpoint: bool,
    pub allow_overlay_network: bool,
    /// The opaque identity the index stores (SPEC §6.4), computed by the configuration.
    pub profile_id: String,
}

impl OllamaEmbedConfig {
    fn llm_config(&self) -> LlmConfig {
        LlmConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            timeout: self.timeout,
            allow_public_endpoint: self.allow_public_endpoint,
            allow_overlay_network: self.allow_overlay_network,
            ..LlmConfig::default()
        }
    }
}

/// Strip `sha256:` and lowercase, so `ollama show` and `/api/tags` spellings compare equal.
pub fn normalise_digest(d: &str) -> String {
    d.trim()
        .strip_prefix("sha256:")
        .unwrap_or(d.trim())
        .to_ascii_lowercase()
}

/// Read a JSON answer of at most [`crate::client::MAX_BODY_BYTES`]. The error is the reason
/// as text, which the callers put after what they expected to get.
async fn read_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> std::result::Result<T, String> {
    use crate::client::{BodyError, MAX_BODY_BYTES, read_body};
    let bytes = read_body(resp, MAX_BODY_BYTES).await.map_err(|e| match e {
        BodyError::Transport(e) => e.without_url().to_string(),
        BodyError::TooLarge(limit) => format!("the body is larger than {limit} bytes; not read"),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// One model as `GET /api/tags` lists it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstalledModel {
    pub name: String,
    pub digest: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<InstalledModel>,
}

#[derive(Deserialize)]
struct ShowResponse {
    #[serde(default)]
    modelfile: String,
    #[serde(default)]
    model_info: serde_json::Map<String, serde_json::Value>,
}

/// What `POST /api/show` says about a model: the weights blob it loads and its embedding
/// length, when it states one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShownModel {
    pub weights_digest: Option<String>,
    pub embedding_length: Option<usize>,
    pub license: Option<String>,
}

fn parse_show(r: ShowResponse) -> ShownModel {
    let weights_digest = r
        .modelfile
        .lines()
        .filter_map(|l| l.trim().strip_prefix("FROM "))
        .filter_map(|p| p.trim().rsplit('/').next())
        .filter_map(|f| f.strip_prefix("sha256-"))
        .map(normalise_digest)
        .next();
    let embedding_length = r
        .model_info
        .iter()
        .find(|(k, _)| k.ends_with(".embedding_length"))
        .and_then(|(_, v)| v.as_u64())
        .map(|n| n as usize);
    let license = r
        .model_info
        .get("general.license")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    ShownModel {
        weights_digest,
        embedding_length,
        license,
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: Vec<String>,
    truncate: bool,
    keep_alive: &'a str,
    options: EmbedOptions,
}

#[derive(Serialize)]
struct EmbedOptions {
    num_ctx: u32,
}

#[derive(Deserialize)]
struct EmbedResponse {
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
}

/// Does `listed` (as Ollama names it) mean the configured `wanted`? Ollama adds `:latest`
/// to a name given without a tag.
pub fn same_model(listed: &str, wanted: &str) -> bool {
    listed == wanted || (!wanted.contains(':') && listed == format!("{wanted}:latest"))
}

enum Job {
    Embed {
        texts: Vec<String>,
        reply: mpsc::Sender<Result<Vec<Vec<f32>>>>,
    },
    Installed {
        reply: mpsc::Sender<Result<Vec<InstalledModel>>>,
    },
    Show {
        model: String,
        reply: mpsc::Sender<Result<ShownModel>>,
    },
}

/// Vectors from a model served by a local Ollama.
pub struct OllamaEmbedder {
    cfg: OllamaEmbedConfig,
    jobs: Mutex<mpsc::Sender<Job>>,
    down_until: Mutex<Option<(Instant, String)>>,
}

impl std::fmt::Debug for OllamaEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaEmbedder")
            .field("base_url", &self.cfg.base_url)
            .field("model", &self.cfg.model)
            .field("profile_id", &self.cfg.profile_id)
            .finish()
    }
}

impl OllamaEmbedder {
    /// Start the worker. No request is made here: the endpoint is validated and the digest
    /// checked by the first call that needs them, so constructing this costs a thread and
    /// never a network round trip (`status` and `write` construct nothing at all).
    pub fn new(
        cfg: OllamaEmbedConfig,
        audit: Arc<dyn AuditSink>,
        gate: Arc<dyn EgressGate>,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let worker_cfg = cfg.clone();
        std::thread::Builder::new()
            .name("cyberbrain-embed-ollama".into())
            .spawn(move || Worker::new(worker_cfg, audit, gate).run(rx))
            .map_err(|e| Error::Embed(format!("could not start the embedding worker: {e}")))?;
        Ok(Self {
            cfg,
            jobs: Mutex::new(tx),
            down_until: Mutex::new(None),
        })
    }

    pub fn config(&self) -> &OllamaEmbedConfig {
        &self.cfg
    }

    fn send<T>(&self, make: impl FnOnce(mpsc::Sender<Result<T>>) -> Job) -> Result<T> {
        let (reply, answer) = mpsc::channel();
        self.jobs
            .lock()
            .map_err(|_| Error::Embed("the embedding worker's queue is poisoned".into()))?
            .send(make(reply))
            .map_err(|_| Error::Embed("the embedding worker has stopped".into()))?;
        answer
            .recv()
            .map_err(|_| Error::Embed("the embedding worker stopped mid-request".into()))?
    }

    /// The models Ollama has installed, with their digests. One audited `GET /api/tags`.
    pub fn installed(&self) -> Result<Vec<InstalledModel>> {
        self.send(|reply| Job::Installed { reply })
    }

    /// What Ollama says about `model`: weights digest, embedding length, licence. One audited
    /// `POST /api/show`.
    pub fn show(&self, model: &str) -> Result<ShownModel> {
        let model = model.to_string();
        self.send(|reply| Job::Show { model, reply })
    }

    fn embed_prefixed(&self, prefix: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if let Ok(guard) = self.down_until.lock()
            && let Some((until, why)) = guard.as_ref()
            && Instant::now() < *until
        {
            return Err(Error::Embed(format!(
                "{why}; not asked again for {} s",
                until.saturating_duration_since(Instant::now()).as_secs()
            )));
        }

        // Degenerate input never leaves the process (SPEC §6.3).
        let mut out = vec![vec![0.0f32; self.cfg.dim]; texts.len()];
        let live: Vec<usize> = (0..texts.len())
            .filter(|&i| !texts[i].trim().is_empty())
            .collect();
        if live.is_empty() {
            return Ok(out);
        }
        let inputs: Vec<String> = live
            .iter()
            .map(|&i| format!("{prefix}{}", texts[i]))
            .collect();
        match self.send(|reply| Job::Embed {
            texts: inputs,
            reply,
        }) {
            Ok(vectors) => {
                for (slot, v) in live.into_iter().zip(vectors) {
                    out[slot] = v;
                }
                Ok(out)
            }
            Err(e) => {
                if let Error::Llm(msg) = &e
                    && msg.contains("unreachable")
                    && let Ok(mut guard) = self.down_until.lock()
                {
                    *guard = Some((Instant::now() + BACKOFF, msg.clone()));
                }
                Err(match e {
                    Error::Llm(msg) => Error::Embed(msg),
                    other => other,
                })
            }
        }
    }
}

impl Embedder for OllamaEmbedder {
    fn dim(&self) -> usize {
        self.cfg.dim
    }

    fn profile_id(&self) -> &str {
        &self.cfg.profile_id
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_prefixed(&self.cfg.document_prefix, texts)
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        Ok(self
            .embed_prefixed(&self.cfg.query_prefix, &[query])?
            .pop()
            .unwrap_or_else(|| vec![0.0; self.cfg.dim]))
    }
}

/// Lives on the worker thread: the runtime, the pinned client, and when the digest was last
/// seen to be the right one.
struct Worker {
    cfg: OllamaEmbedConfig,
    audit: Arc<dyn AuditSink>,
    gate: Arc<dyn EgressGate>,
    conn: Option<(ValidatedEndpoint, reqwest::Client)>,
    /// When the check last ended, and how: `Ok` lets embedding go ahead, `Err` is handed back
    /// unchanged until [`VERIFY_EVERY`] has passed.
    verified: Option<(Instant, std::result::Result<(), String>)>,
}

impl Worker {
    fn new(cfg: OllamaEmbedConfig, audit: Arc<dyn AuditSink>, gate: Arc<dyn EgressGate>) -> Self {
        Self {
            cfg,
            audit,
            gate,
            conn: None,
            verified: None,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Job>) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let msg = format!("could not start the embedding runtime: {e}");
                for job in rx {
                    match job {
                        Job::Embed { reply, .. } => {
                            let _ = reply.send(Err(Error::Embed(msg.clone())));
                        }
                        Job::Installed { reply } => {
                            let _ = reply.send(Err(Error::Embed(msg.clone())));
                        }
                        Job::Show { reply, .. } => {
                            let _ = reply.send(Err(Error::Embed(msg.clone())));
                        }
                    }
                }
                return;
            }
        };
        for job in rx {
            match job {
                Job::Embed { texts, reply } => {
                    let r = rt.block_on(self.embed(texts));
                    let _ = reply.send(r);
                }
                Job::Installed { reply } => {
                    let r = rt.block_on(self.installed());
                    let _ = reply.send(r);
                }
                Job::Show { model, reply } => {
                    let r = rt.block_on(self.show(&model));
                    let _ = reply.send(r);
                }
            }
        }
    }

    fn record(
        &self,
        call: CallKind,
        url: &str,
        outcome: CallOutcome,
        usage: Option<Usage>,
        started: Instant,
    ) {
        let (resolved, public_waived) = match &self.conn {
            Some((ep, _)) => (ep.addrs.clone(), ep.public_waived),
            None => (Vec::new(), false),
        };
        self.audit.record_inference(InferenceEvent {
            at: jiff::Timestamp::now(),
            purpose: EgressPurpose::LocalInference,
            call,
            task: Some("embedding"),
            endpoint: url.to_string(),
            resolved,
            public_waived,
            model: self.cfg.model.clone(),
            outcome,
            usage,
            elapsed: started.elapsed(),
        });
    }

    async fn connect(&mut self) -> Result<()> {
        if self.conn.is_some() {
            return Ok(());
        }
        let started = Instant::now();
        let policy = EndpointPolicy {
            allow_public_endpoint: self.cfg.allow_public_endpoint,
            allow_overlay_network: self.cfg.allow_overlay_network,
            ..Default::default()
        };
        let endpoint =
            match crate::address::validate_endpoint(&self.cfg.base_url, &policy, &SystemResolver)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    self.record(
                        CallKind::EndpointValidation,
                        &self.cfg.base_url,
                        match &e {
                            Error::PolicyRefusal { reason, .. } => {
                                CallOutcome::Refused(reason.clone())
                            }
                            other => CallOutcome::Unreachable(other.to_string()),
                        },
                        None,
                        started,
                    );
                    return Err(e);
                }
            };
        let http = build_http_client(
            self.gate.as_ref(),
            EgressPurpose::LocalInference,
            &endpoint,
            &self.cfg.llm_config(),
        )?;
        self.conn = Some((endpoint, http));
        self.record(
            CallKind::EndpointValidation,
            &self.cfg.base_url,
            CallOutcome::Ok,
            None,
            started,
        );
        Ok(())
    }

    fn url(&self, route: &str) -> String {
        let base = match &self.conn {
            Some((ep, _)) => ep.base_url.as_str().trim_end_matches('/').to_string(),
            None => self.cfg.base_url.trim_end_matches('/').to_string(),
        };
        format!("{base}/{route}")
    }

    fn authorise(&self) -> Result<()> {
        let summary = match &self.conn {
            Some((ep, _)) => ep.summary(),
            None => self.cfg.base_url.clone(),
        };
        self.gate.permit(EgressPurpose::LocalInference, &summary)
    }

    async fn installed(&mut self) -> Result<Vec<InstalledModel>> {
        self.connect().await?;
        let url = self.url("api/tags");
        let started = Instant::now();
        self.authorise()?;
        let http = self.conn.as_ref().expect("connected").1.clone();
        let resp = http.get(&url).send().await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(CallKind::VendorStatus, &url, outcome, None, started);
                return Err(err);
            }
        };
        let status = resp.status();
        if !status.is_success() {
            self.record(
                CallKind::VendorStatus,
                &url,
                CallOutcome::BadStatus(status.as_u16()),
                None,
                started,
            );
            return Err(Error::Embed(format!(
                "{url} answered {status}; is this an Ollama server?"
            )));
        }
        match read_json::<TagsResponse>(resp).await {
            Ok(t) => {
                self.record(CallKind::VendorStatus, &url, CallOutcome::Ok, None, started);
                Ok(t.models)
            }
            Err(e) => {
                self.record(
                    CallKind::VendorStatus,
                    &url,
                    CallOutcome::BadResponse(e.to_string()),
                    None,
                    started,
                );
                Err(Error::Embed(format!(
                    "{url} sent a model list that does not parse: {e}"
                )))
            }
        }
    }

    async fn show(&mut self, model: &str) -> Result<ShownModel> {
        self.connect().await?;
        let url = self.url("api/show");
        let started = Instant::now();
        self.authorise()?;
        let http = self.conn.as_ref().expect("connected").1.clone();
        let resp = match http
            .post(&url)
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(CallKind::VendorStatus, &url, outcome, None, started);
                return Err(err);
            }
        };
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            self.record(
                CallKind::VendorStatus,
                &url,
                CallOutcome::BadStatus(404),
                None,
                started,
            );
            return Err(Error::Embed(format!(
                "{model} is not installed in the Ollama at {}; `ollama pull {model}` fetches it \
                 (Cyberbrain downloads nothing itself)",
                self.cfg.base_url
            )));
        }
        if !status.is_success() {
            self.record(
                CallKind::VendorStatus,
                &url,
                CallOutcome::BadStatus(status.as_u16()),
                None,
                started,
            );
            return Err(Error::Embed(format!(
                "{url} answered {status}; is this an Ollama server?"
            )));
        }
        match read_json::<ShowResponse>(resp).await {
            Ok(r) => {
                self.record(CallKind::VendorStatus, &url, CallOutcome::Ok, None, started);
                Ok(parse_show(r))
            }
            Err(e) => {
                self.record(
                    CallKind::VendorStatus,
                    &url,
                    CallOutcome::BadResponse(e.to_string()),
                    None,
                    started,
                );
                Err(Error::Embed(format!(
                    "{url} sent a model description that does not parse: {e}"
                )))
            }
        }
    }

    /// The configured digest and dimension are those of the weights Ollama loads for the
    /// configured model, or an error that says what it loads instead.
    async fn verify(&mut self) -> Result<()> {
        if let Some((at, outcome)) = &self.verified
            && at.elapsed() < VERIFY_EVERY
        {
            return outcome.clone().map_err(Error::Embed);
        }
        let model = self.cfg.model.clone();
        let outcome = match self.show(&model).await {
            Err(Error::Llm(msg)) if msg.contains("unreachable") => {
                // Reachability is the caller's backoff, not a verdict about the model.
                return Err(Error::Llm(msg));
            }
            Err(e) => Err(e.to_string()),
            Ok(shown) => self.judge(&shown),
        };
        self.verified = Some((Instant::now(), outcome.clone()));
        outcome.map_err(Error::Embed)
    }

    fn judge(&self, shown: &ShownModel) -> std::result::Result<(), String> {
        let wanted = normalise_digest(&self.cfg.digest);
        let Some(served) = &shown.weights_digest else {
            return Err(format!(
                "Ollama did not say which weights it loads for {} (no FROM line in /api/show), \
                 so they cannot be checked against embedding.ollama.digest; refusing",
                self.cfg.model
            ));
        };
        if *served != wanted {
            return Err(format!(
                "Ollama loads {} from weights {served}, the configuration pins {wanted}; the \
                 weights behind the tag changed and their vectors would not be comparable with \
                 the stored ones. If the change is intended, set embedding.ollama.digest to the \
                 new value and run `cyberbrain scan`, which re-embeds everything",
                self.cfg.model
            ));
        }
        if let Some(len) = shown.embedding_length
            && len != self.cfg.dim
        {
            return Err(format!(
                "{} has embedding length {len}, embedding.ollama.dim says {}",
                self.cfg.model, self.cfg.dim
            ));
        }
        Ok(())
    }

    async fn embed(&mut self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.connect().await?;
        self.verify().await?;
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.cfg.batch.max(1)) {
            out.extend(self.embed_chunk(chunk.to_vec()).await?);
        }
        Ok(out)
    }

    async fn embed_chunk(&mut self, input: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let url = self.url("api/embed");
        let started = Instant::now();
        let n = input.len();
        self.authorise()?;
        let http = self.conn.as_ref().expect("connected").1.clone();
        let body = EmbedRequest {
            model: &self.cfg.model,
            input,
            // A block longer than the context is cut, not refused: refusing would drop the
            // whole note from semantic search over its tail.
            truncate: true,
            keep_alive: &self.cfg.keep_alive,
            options: EmbedOptions {
                num_ctx: self.cfg.num_ctx,
            },
        };
        let resp = match http.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(CallKind::Embedding, &url, outcome, None, started);
                return Err(err);
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let text = crate::client::read_error_body(resp).await;
            self.record(
                CallKind::Embedding,
                &url,
                CallOutcome::BadStatus(status.as_u16()),
                None,
                started,
            );
            return Err(Error::Embed(format!(
                "{url} answered {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let parsed: EmbedResponse = match read_json(resp).await {
            Ok(p) => p,
            Err(e) => {
                self.record(
                    CallKind::Embedding,
                    &url,
                    CallOutcome::BadResponse(e.to_string()),
                    None,
                    started,
                );
                return Err(Error::Embed(format!(
                    "{url} sent vectors that do not parse: {e}"
                )));
            }
        };
        let usage = Some(Usage {
            prompt_tokens: parsed.prompt_eval_count,
            completion_tokens: None,
            total_tokens: parsed.prompt_eval_count,
            cached_prompt_tokens: None,
        });
        let bad = if parsed.embeddings.len() != n {
            Some(format!(
                "{} vectors for {n} inputs",
                parsed.embeddings.len()
            ))
        } else {
            parsed
                .embeddings
                .iter()
                .find(|v| v.len() != self.cfg.dim)
                .map(|v| {
                    format!(
                        "a vector of dimension {} where embedding.ollama.dim says {}",
                        v.len(),
                        self.cfg.dim
                    )
                })
                .or_else(|| {
                    parsed
                        .embeddings
                        .iter()
                        .any(|v| v.iter().any(|x| !x.is_finite()))
                        .then(|| "a vector with a non-finite component".to_string())
                })
        };
        if let Some(what) = bad {
            self.record(
                CallKind::Embedding,
                &url,
                CallOutcome::BadResponse(what.clone()),
                usage,
                started,
            );
            return Err(Error::Embed(format!("{url} returned {what}")));
        }
        self.record(CallKind::Embedding, &url, CallOutcome::Ok, usage, started);
        Ok(parsed
            .embeddings
            .into_iter()
            .map(|mut v| {
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    v.iter_mut().for_each(|x| *x /= norm);
                }
                v
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemoryAuditSink;
    use crate::mock::{MockResponse, MockServer};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct OpenGate;
    impl EgressGate for OpenGate {
        fn permit(&self, _: EgressPurpose, _: &str) -> Result<()> {
            Ok(())
        }
    }

    const DIGEST: &str = "9284ddb28f3e32696e612d3bf6bf9666c560c0aa6106f8be01f71d147d5c5cb2";

    fn cfg(base_url: String, digest: &str) -> OllamaEmbedConfig {
        OllamaEmbedConfig {
            base_url,
            model: "gemma-test".into(),
            digest: digest.into(),
            dim: 3,
            query_prefix: "Q: ".into(),
            document_prefix: "D: ".into(),
            num_ctx: 512,
            batch: 2,
            timeout: Duration::from_secs(5),
            keep_alive: "1m".into(),
            allow_public_endpoint: false,
            allow_overlay_network: false,
            profile_id: "test-profile".into(),
        }
    }

    /// An Ollama that loads `served` and answers `embed` with `[3, 0, 4]` per input (norm 5),
    /// recording the inputs it was sent.
    async fn ollama(
        served: &'static str,
        shows: Arc<AtomicUsize>,
        inputs: Arc<Mutex<Vec<String>>>,
    ) -> MockServer {
        MockServer::start(move |req| match req.path.as_str() {
            "/api/show" => {
                shows.fetch_add(1, Ordering::SeqCst);
                MockResponse::json(
                    200,
                    &serde_json::json!({
                        "modelfile": format!("FROM /m/blobs/sha256-{served}\n"),
                        "model_info": { "x.embedding_length": 3 }
                    })
                    .to_string(),
                )
            }
            "/api/embed" => {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let input: Vec<String> = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
                let n = input.len();
                inputs.lock().unwrap().extend(input);
                let vecs: Vec<[f32; 3]> = (0..n).map(|_| [3.0, 0.0, 4.0]).collect();
                MockResponse::json(
                    200,
                    &serde_json::json!({ "embeddings": vecs, "prompt_eval_count": 7 }).to_string(),
                )
            }
            _ => MockResponse::json(404, "{}"),
        })
        .await
    }

    /// Would fail if an Ollama answering with an endless body were read into memory whole.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_oversized_embedding_answer_is_refused_not_read() {
        let server = MockServer::start(move |req| match req.path.as_str() {
            "/api/show" => MockResponse::json(
                200,
                &serde_json::json!({
                    "modelfile": format!("FROM /m/blobs/sha256-{DIGEST}\n"),
                    "model_info": { "x.embedding_length": 3 }
                })
                .to_string(),
            ),
            "/api/embed" => MockResponse::new(200, vec![b' '; 16 * 1024 * 1024 + 1]),
            _ => MockResponse::json(404, "{}"),
        })
        .await;
        let e = OllamaEmbedder::new(
            cfg(format!("http://{}", server.addr), DIGEST),
            MemoryAuditSink::new(),
            Arc::new(OpenGate),
        )
        .unwrap();
        let err = tokio::task::spawn_blocking(move || e.embed(&["a"]).unwrap_err())
            .await
            .unwrap();
        assert!(err.to_string().contains("larger than"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embeds_with_the_right_prefixes_normalises_and_audits() {
        let shows = Arc::new(AtomicUsize::new(0));
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let server = ollama(DIGEST, shows.clone(), inputs.clone()).await;
        let audit = MemoryAuditSink::new();
        let e = OllamaEmbedder::new(
            cfg(
                format!("http://{}", server.addr),
                &format!("sha256:{DIGEST}"),
            ),
            audit.clone(),
            Arc::new(OpenGate),
        )
        .unwrap();
        let (docs, query) = tokio::task::spawn_blocking(move || {
            (
                e.embed(&["a", "  ", "b", "c"]).unwrap(),
                e.embed_query("q").unwrap(),
            )
        })
        .await
        .unwrap();
        assert_eq!(docs.len(), 4);
        assert_eq!(docs[0], vec![0.6, 0.0, 0.8]);
        assert_eq!(
            docs[1],
            vec![0.0, 0.0, 0.0],
            "blank input is the zero vector"
        );
        assert_eq!(query, vec![0.6, 0.0, 0.8]);
        assert_eq!(
            *inputs.lock().unwrap(),
            vec!["D: a", "D: b", "D: c", "Q: q"],
            "blank input is never sent; documents and queries get their own prefix"
        );
        assert_eq!(
            shows.load(Ordering::SeqCst),
            1,
            "the digest is checked once, then trusted for a while"
        );
        let embeds = audit
            .rows()
            .into_iter()
            .filter(|r| r.call == CallKind::Embedding)
            .count();
        assert_eq!(
            embeds, 3,
            "batch 2: two requests for three blocks, one for the query"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn other_weights_are_refused_and_the_refusal_is_remembered() {
        let shows = Arc::new(AtomicUsize::new(0));
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let server = ollama(
            "0000000000000000000000000000000000000000000000000000000000000000",
            shows.clone(),
            inputs.clone(),
        )
        .await;
        let e = OllamaEmbedder::new(
            cfg(format!("http://{}", server.addr), DIGEST),
            Arc::new(MemoryAuditSink::default()),
            Arc::new(OpenGate),
        )
        .unwrap();
        let (a, b) = tokio::task::spawn_blocking(move || (e.embed(&["a"]), e.embed(&["b"])))
            .await
            .unwrap();
        let msg = a.unwrap_err().to_string();
        assert!(msg.contains("pins"), "{msg}");
        assert!(b.is_err());
        assert_eq!(
            shows.load(Ordering::SeqCst),
            1,
            "a failed check is not repeated per note"
        );
        assert!(
            inputs.lock().unwrap().is_empty(),
            "nothing was embedded with the wrong weights"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unreachable_ollama_is_not_asked_again_at_once() {
        // A port nobody listens on: bind, note it, close.
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let audit = MemoryAuditSink::new();
        let e = OllamaEmbedder::new(
            cfg(format!("http://{addr}"), DIGEST),
            audit.clone(),
            Arc::new(OpenGate),
        )
        .unwrap();
        let (a, b) = tokio::task::spawn_blocking(move || (e.embed(&["a"]), e.embed(&["b"])))
            .await
            .unwrap();
        assert!(a.unwrap_err().to_string().contains("unreachable"));
        let after_first = audit.len();
        let msg = b.unwrap_err().to_string();
        assert!(msg.contains("not asked again"), "{msg}");
        assert_eq!(audit.len(), after_first, "the second call made no request");
    }

    #[test]
    fn digests_compare_across_spellings() {
        assert_eq!(normalise_digest("sha256:ABCdef"), "abcdef");
        assert_eq!(normalise_digest(" abcdef "), "abcdef");
    }

    #[test]
    fn the_weights_digest_comes_from_the_from_line() {
        let r: ShowResponse = serde_json::from_value(serde_json::json!({
            "modelfile": "# Modelfile generated by \"ollama show\"\nFROM /root/.ollama/models/blobs/sha256-9284DDB28f3e\nTEMPLATE {{ .Prompt }}\n",
            "model_info": { "gemma-embedding.embedding_length": 768, "general.license": "gemma" }
        }))
        .unwrap();
        let shown = parse_show(r);
        assert_eq!(shown.weights_digest.as_deref(), Some("9284ddb28f3e"));
        assert_eq!(shown.embedding_length, Some(768));
        assert_eq!(shown.license.as_deref(), Some("gemma"));
    }

    #[test]
    fn no_from_line_means_no_digest() {
        let r: ShowResponse =
            serde_json::from_value(serde_json::json!({ "modelfile": "TEMPLATE x" })).unwrap();
        assert_eq!(parse_show(r).weights_digest, None);
    }

    #[test]
    fn an_untagged_name_means_latest() {
        assert!(same_model("bge-m3:latest", "bge-m3"));
        assert!(same_model("embeddinggemma:300m", "embeddinggemma:300m"));
        assert!(!same_model(
            "embeddinggemma:300m",
            "embeddinggemma:300m-qat-q4_0"
        ));
        assert!(!same_model(
            "embeddinggemma:300m-qat-q4_0",
            "embeddinggemma"
        ));
    }
}
