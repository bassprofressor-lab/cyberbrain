//! The audit hook. SPEC §11 requires the model, endpoint and token counts of every call to
//! be recorded; §12.6 says the audit log is owned by the policy layer. This module defines
//! the shape of what is recorded and the trait that receives it. It stores nothing.
//!
//! What the policy crate needs to implement: `impl AuditSink for YourLog` with one method,
//! `record_inference(&self, event: InferenceEvent)`. The method is synchronous and must
//! not block for long; buffer if the write is slow.

use cyberbrain_core::EgressPurpose;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::types::Usage;

/// Which route was called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallKind {
    /// `POST /chat/completions`, non-streaming.
    ChatCompletion,
    /// `POST /chat/completions` with `stream: true`.
    ChatCompletionStream,
    /// `GET /models`.
    ListModels,
    /// A vendor route outside the OpenAI surface, asked only of a backend that was
    /// fingerprinted first. Named so the audit log shows it left the two-route contract.
    VendorStatus,
    /// No request was sent: the endpoint was validated (or refused) before any I/O.
    EndpointValidation,
}

/// How the call ended. Every variant names its side of the boundary (SPEC §14.3): `Ok`
/// means a well-formed answer came back, not that a request went out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "detail")]
pub enum CallOutcome {
    Ok,
    /// Policy refused before any bytes left the process.
    Refused(String),
    /// Could not connect, or the connection dropped.
    Unreachable(String),
    /// The configured timeout elapsed.
    TimedOut,
    /// HTTP status outside 2xx.
    BadStatus(u16),
    /// A 2xx that we could not interpret.
    BadResponse(String),
}

impl CallOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, CallOutcome::Ok)
    }
}

/// One audit row per call. Token counts are `None` when the endpoint did not report them;
/// they are never estimated here, because an estimated count in an audit log reads as a
/// measured one.
#[derive(Debug, Clone, Serialize)]
pub struct InferenceEvent {
    pub at: jiff::Timestamp,
    pub purpose: EgressPurpose,
    pub call: CallKind,
    /// Which spec feature made the call (`"session-summary"` etc.), if any.
    pub task: Option<&'static str>,
    /// Full URL that was (or would have been) requested.
    pub endpoint: String,
    /// The addresses the client was pinned to. Empty on a refusal before resolution.
    pub resolved: Vec<SocketAddr>,
    /// The operator set `allow_public_endpoint` and at least one address needed it.
    pub public_waived: bool,
    /// Model name as sent. Empty for `ListModels`.
    pub model: String,
    pub outcome: CallOutcome,
    pub usage: Option<Usage>,
    pub elapsed: Duration,
}

/// Receives audit rows. Implemented by the policy crate over its append-only table.
pub trait AuditSink: Send + Sync {
    fn record_inference(&self, event: InferenceEvent);
}

/// Discards everything. Only for callers that have no audit log at all, which in the
/// finished binary is nobody; it exists so the crate can be used in isolation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn record_inference(&self, _event: InferenceEvent) {}
}

/// Keeps rows in memory. Used by tests and usable by a status screen.
#[derive(Debug, Default)]
pub struct MemoryAuditSink {
    rows: Mutex<Vec<InferenceEvent>>,
}

impl MemoryAuditSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn rows(&self) -> Vec<InferenceEvent> {
        self.rows.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.rows.lock().map(|r| r.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AuditSink for MemoryAuditSink {
    fn record_inference(&self, event: InferenceEvent) {
        if let Ok(mut rows) = self.rows.lock() {
            rows.push(event);
        }
    }
}

impl<T: AuditSink + ?Sized> AuditSink for Arc<T> {
    fn record_inference(&self, event: InferenceEvent) {
        (**self).record_inference(event)
    }
}
