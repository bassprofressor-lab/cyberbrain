//! Cyberbrain compliance layer (SPEC §12).
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2. Implemented from
//! `docs/SPEC.md` alone under the clean-room rule in §0.
//!
//! This crate is load-bearing, not paperwork. It owns:
//!
//! - the **egress register** ([`egress`]) and [`Egress`], the implementation of
//!   [`cyberbrain_core::EgressGate`] that every outbound request must be permitted by;
//!   with the `http` feature it also owns the model-download transport;
//! - the compliance **profiles** `eu`, `ch`, `off` and the obligations each encodes, with an
//!   explicit confidence level on every claim ([`profile`]);
//! - the write-time **PII scan** and its hold/redact/review flow ([`pii`], [`write_gate`]);
//! - **retention** evaluation and its explicit, never-in-the-background apply path
//!   ([`retention`]);
//! - the policy side of **erasure** and **subject access**, and the trait seams the index
//!   crate and the binary must satisfy for the storage side ([`erasure`], [`subject`]);
//! - the **audit log** writer with a verifiable hash chain ([`audit`]) and the **model card**
//!   output ([`model_card`]).
//!
//! [`Policy`] is the facade the CLI and the web UI call; every module is also usable alone.
//!
//! # Profiles
//!
//! `off` disables the *compliance checks* (PII scan, write hold). It does **not** disable the
//! egress gate or the audit log: local-first is a defining property of the product (SPEC §1)
//! and is not a compliance setting. Under `off` the hot path costs one enum comparison.

pub mod audit;
pub mod config;
pub mod egress;
pub mod erasure;
pub mod model_card;
pub mod pii;
mod policy;
pub mod profile;
pub mod retention;
pub mod subject;
pub mod write_gate;

pub use audit::{
    Actor, AuditAction, AuditEvent, AuditFilter, AuditLog, AuditSink, ExportFormat,
    MemoryAuditSink, bundle, verify_chain_from,
};
pub use config::PolicyConfig;
pub use egress::{Egress, EgressEntry, EgressTicket, Locality};
pub use erasure::{EraseReason, EraseRequest, Eraser, ErasureReport};
pub use model_card::{ModelCard, ModelInventory, ModelRole};
pub use pii::{Finding, PiiKind};
pub use policy::{Policy, PolicyStatus};
pub use profile::{ALL_PROFILES, Obligation, Profile, ProfileExt, Topic, parse_profile};
pub use retention::{RetentionItem, RetentionQueue, RetentionStatus};
pub use subject::{Identifier, SubjectAccessReport, SubjectBlock, SubjectSource};
pub use write_gate::{OperatorChoice, ResolvedWrite, ScanStatus, WriteVerdict};

use serde::{Deserialize, Serialize};

/// How sure the code is about something it states. Used both for compliance claims (is this
/// really what the law says?) and for PII detections (is this really an IBAN?).
///
/// An overconfident claim is worse than an absent one, so the level is carried with the
/// data and printed with it. Nothing in this crate may present a `Low` as a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
