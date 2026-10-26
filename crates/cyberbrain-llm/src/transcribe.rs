//! Speech to text through a local whisper.cpp server (`whisper-server`).
//!
//! The model runs in its own process, as embeddings run in Ollama: no C++ enters this
//! build, the Windows installer stays what it is, and the same rule holds as for every
//! other endpoint here: loopback or private addresses only unless the operator waived it,
//! the validated addresses pinned, no proxy, no redirect, every call in the audit log.
//!
//! One route: `POST {base}/inference`, multipart, the audio as `file`. The server decodes
//! anything but 16 kHz WAV only when it was started with `--convert` (ffmpeg on its side);
//! a browser recording is WebM/Opus, so the dictation button needs that flag.
//!
//! The decoding settings are sent with every request rather than left to how the server
//! was started, because they are the ones that were measured: on German speech
//! large-v3-turbo made 5.9 % word errors with the defaults and 2.0 % with the previous
//! text not carried as context (`max_context=0`), the difference being hallucinated
//! insertions and loops, not mishearings. Temperature 0 keeps a transcript repeatable.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::address::{EndpointPolicy, SystemResolver, ValidatedEndpoint};
use crate::audit::{AuditSink, CallKind, CallOutcome, InferenceEvent};
use crate::client::{LlmConfig, build_http_client, read_body, read_error_body, transport_failure};
use cyberbrain_core::{EgressGate, EgressPurpose, Error, Result};

/// The longest answer read back. An hour of speech is some 60 KB of text; segments in
/// `verbose_json` multiply that by a few. Past this the server is misbehaving.
const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024 * 1024;

/// What `[transcription]` resolves to.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Base URL of `whisper-server`, without `/inference`.
    pub base_url: String,
    /// Spoken language, `de` or another whisper code; `auto` lets the model guess.
    pub language: String,
    /// Shown in the audit log and the note's header. whisper-server serves the one model
    /// it was started with, so this names it rather than choosing it.
    pub model_label: String,
    /// Whole-request timeout. Transcription is slow on a CPU: an hour of audio took
    /// 15 minutes with large-v3-turbo on 12 EPYC cores.
    pub timeout: Duration,
    pub allow_public_endpoint: bool,
    pub allow_overlay_network: bool,
}

impl WhisperConfig {
    fn llm_config(&self) -> LlmConfig {
        LlmConfig {
            base_url: self.base_url.clone(),
            model: self.model_label.clone(),
            timeout: self.timeout,
            allow_public_endpoint: self.allow_public_endpoint,
            allow_overlay_network: self.allow_overlay_network,
            ..LlmConfig::default()
        }
    }
}

/// One stretch of speech with its place in the recording, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transcript {
    pub text: String,
    /// The language the server reports, which is the requested one unless it was `auto`.
    pub language: Option<String>,
    /// Length of the audio in seconds, as decoded by the server.
    pub duration_s: Option<f64>,
    pub segments: Vec<Segment>,
    pub model: String,
    pub elapsed_ms: u64,
}

#[derive(Deserialize)]
struct WireTranscript {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    segments: Vec<Segment>,
}

/// A validated, pinned, audited connection to one whisper-server.
pub struct WhisperClient {
    cfg: WhisperConfig,
    endpoint: ValidatedEndpoint,
    http: reqwest::Client,
    audit: Arc<dyn AuditSink>,
    gate: Arc<dyn EgressGate>,
}

impl WhisperClient {
    /// Validate the address and build the client. A refusal is audited, then returned.
    pub async fn connect(
        cfg: WhisperConfig,
        audit: Arc<dyn AuditSink>,
        gate: Arc<dyn EgressGate>,
    ) -> Result<Self> {
        let started = Instant::now();
        let policy = EndpointPolicy {
            allow_public_endpoint: cfg.allow_public_endpoint,
            allow_overlay_network: cfg.allow_overlay_network,
            ..Default::default()
        };
        let endpoint = match crate::address::validate_endpoint(
            &cfg.base_url,
            &policy,
            &SystemResolver,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                let outcome = match &e {
                    Error::PolicyRefusal { reason, .. } => CallOutcome::Refused(reason.clone()),
                    other => CallOutcome::Unreachable(other.to_string()),
                };
                audit.record_inference(event(
                    &cfg,
                    CallKind::EndpointValidation,
                    &cfg.base_url,
                    &[],
                    false,
                    outcome,
                    started,
                ));
                return Err(e);
            }
        };
        let http = build_http_client(
            gate.as_ref(),
            EgressPurpose::LocalInference,
            &endpoint,
            &cfg.llm_config(),
        )?;
        audit.record_inference(event(
            &cfg,
            CallKind::EndpointValidation,
            endpoint.base_url.as_str(),
            &endpoint.addrs,
            endpoint.public_waived,
            CallOutcome::Ok,
            started,
        ));
        Ok(Self {
            cfg,
            endpoint,
            http,
            audit,
            gate,
        })
    }

    pub fn config(&self) -> &WhisperConfig {
        &self.cfg
    }

    pub fn endpoint(&self) -> &ValidatedEndpoint {
        &self.endpoint
    }

    /// Send one recording and read the transcript. `file_name` and `mime` only label the
    /// multipart part; the server decides by content what it can decode.
    pub async fn transcribe(
        &self,
        audio: &[u8],
        file_name: &str,
        mime: &str,
    ) -> Result<Transcript> {
        let url = format!(
            "{}/inference",
            self.endpoint.base_url.as_str().trim_end_matches('/')
        );
        let started = Instant::now();
        self.gate
            .permit(EgressPurpose::LocalInference, &self.endpoint.summary())?;
        let (boundary, body) = multipart(
            &[
                ("language", self.cfg.language.as_str()),
                ("max_context", "0"),
                ("temperature", "0.0"),
                ("response_format", "verbose_json"),
            ],
            audio,
            file_name,
            mime,
        );
        let resp = self
            .http
            .post(&url)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let (outcome, err) = transport_failure(&url, &e);
                self.record(&url, outcome, started);
                return Err(err);
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let text = read_error_body(resp).await;
            let msg = format!(
                "{url} answered {status}: {}",
                text.trim().chars().take(200).collect::<String>()
            );
            self.record(&url, CallOutcome::BadResponse(msg.clone()), started);
            return Err(Error::Llm(msg));
        }
        let bytes = match read_body(resp, MAX_TRANSCRIPT_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                let (outcome, err) = e.outcome(&url);
                self.record(&url, outcome, started);
                return Err(err);
            }
        };
        let wire: WireTranscript = match serde_json::from_slice(&bytes) {
            Ok(w) => w,
            Err(e) => {
                // whisper-server answers 200 with {"error": "..."} when it cannot decode
                // the audio, which is the usual way a WebM upload fails without --convert.
                let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                    .unwrap_or_else(|| format!("not a transcript: {e}"));
                let msg = format!("{url}: {detail}");
                self.record(&url, CallOutcome::BadResponse(msg.clone()), started);
                return Err(Error::Llm(msg));
            }
        };
        self.record(&url, CallOutcome::Ok, started);
        Ok(Transcript {
            // whisper-server joins its segments with line breaks that fall mid-sentence;
            // the text is one flow, and `segments` keeps where each stretch began.
            text: wire.text.split_whitespace().collect::<Vec<_>>().join(" "),
            language: wire.language,
            duration_s: wire.duration,
            segments: wire
                .segments
                .into_iter()
                .map(|s| Segment {
                    text: s.text.trim().to_string(),
                    ..s
                })
                .collect(),
            model: self.cfg.model_label.clone(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    fn record(&self, url: &str, outcome: CallOutcome, started: Instant) {
        self.audit.record_inference(event(
            &self.cfg,
            CallKind::Transcription,
            url,
            &self.endpoint.addrs,
            self.endpoint.public_waived,
            outcome,
            started,
        ));
    }
}

fn event(
    cfg: &WhisperConfig,
    call: CallKind,
    endpoint: &str,
    resolved: &[std::net::SocketAddr],
    public_waived: bool,
    outcome: CallOutcome,
    started: Instant,
) -> InferenceEvent {
    InferenceEvent {
        at: jiff::Timestamp::now(),
        purpose: EgressPurpose::LocalInference,
        call,
        task: Some("transcription"),
        endpoint: endpoint.to_string(),
        resolved: resolved.to_vec(),
        public_waived,
        model: cfg.model_label.clone(),
        outcome,
        usage: None,
        elapsed: started.elapsed(),
    }
}

/// A `multipart/form-data` body by hand: four text fields and one file are not worth a
/// new reqwest feature and the crates it brings. The boundary is lengthened until it
/// occurs nowhere in the audio, so no recording can end the part early.
fn multipart(
    fields: &[(&str, &str)],
    file: &[u8],
    file_name: &str,
    mime: &str,
) -> (String, Vec<u8>) {
    let mut boundary = String::from("cyberbrain-whisper-7f3a9c");
    while contains(file, boundary.as_bytes()) {
        boundary.push_str("-x");
    }
    let name: String = file_name
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut out = Vec::with_capacity(file.len() + 1024);
    for (k, v) in fields {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(file);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, out)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_carries_fields_and_file() {
        let (b, body) = multipart(&[("language", "de")], b"RIFFdata", "a\"b.wav", "audio/wav");
        let s = String::from_utf8_lossy(&body);
        assert!(s.starts_with(&format!("--{b}\r\n")));
        assert!(s.contains("name=\"language\"\r\n\r\nde\r\n"));
        assert!(s.contains("filename=\"a_b.wav\""));
        assert!(s.contains("Content-Type: audio/wav\r\n\r\nRIFFdata\r\n"));
        assert!(s.ends_with(&format!("--{b}--\r\n")));
    }

    #[test]
    fn boundary_never_occurs_in_the_audio() {
        let audio = b"xx--cyberbrain-whisper-7f3a9cyy";
        let (b, _) = multipart(&[], audio, "f", "audio/wav");
        assert!(!contains(audio, b.as_bytes()));
    }

    #[test]
    fn verbose_json_parses() {
        let w: WireTranscript = serde_json::from_str(
            r#"{"task":"transcribe","language":"german","duration":3.5,"text":" Hallo Welt.",
               "segments":[{"id":0,"start":0.0,"end":3.5,"text":" Hallo Welt.","tokens":[1]}]}"#,
        )
        .unwrap();
        assert_eq!(w.text.trim(), "Hallo Welt.");
        assert_eq!(w.segments.len(), 1);
        assert_eq!(w.duration, Some(3.5));
    }

    #[test]
    fn mid_sentence_breaks_are_flattened() {
        let raw = "Woher ein Modell hat, was es zu wissen\n scheint.\n Ein Satz.";
        assert_eq!(
            raw.split_whitespace().collect::<Vec<_>>().join(" "),
            "Woher ein Modell hat, was es zu wissen scheint. Ein Satz."
        );
    }
}
