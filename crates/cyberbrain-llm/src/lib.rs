//! Cyberbrain llm: the optional local-inference layer (SPEC §11).
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2. Implemented from
//! `docs/SPEC.md` alone under the clean-room boundary in SPEC §0.
//!
//! # What this crate is
//!
//! A client for an **OpenAI-compatible** chat endpoint on the operator's own machine or
//! network, plus the four features the spec builds on top of it: session summaries,
//! contradiction detection, ring/tag suggestion, and note supersession. There is no
//! vendor-specific code: Ollama, LM Studio, llama.cpp, vLLM and NVIDIA PAIR are all reached
//! through the same two routes, `POST {base}/chat/completions` and `GET {base}/models`.
//!
//! # The three properties this crate must never trade away
//!
//! 1. **Nothing leaves the local network unless the operator said so.** Before a client is
//!    built, the configured base URL is resolved and *every* address it resolves to must be
//!    loopback or private ([`address`]). The validated addresses are then pinned into the
//!    HTTP client, so a later DNS answer cannot redirect the connection. Environment proxies
//!    are ignored and HTTP redirects are not followed, because both are ways for bytes to
//!    reach a host that was never validated. Refusal is [`Error::PolicyRefusal`].
//!
//! 2. **Every call is audited.** Endpoint, resolved addresses, model, token counts and
//!    outcome go to an [`audit::AuditSink`] that the policy crate implements. Refusals are
//!    audited too. This crate defines the trait and depends on nothing that stores it.
//!
//! 3. **Everything degrades, nothing blocks.** A missing, refusing, slow or broken endpoint
//!    turns into a [`degrade::Degraded`] carrying a caveat sentence for
//!    `RecallResult::caveats`. There is no fallback to any other provider; there is no
//!    other provider to fall back to.
//!
//! # Egress register
//!
//! The single point where an HTTP client is constructed is [`client::build_http_client`],
//! which takes [`EgressPurpose::LocalInference`] by value. That is the seam the policy
//! crate's register (SPEC §12.1) hooks into. Nothing else in this crate constructs a client.

pub mod address;
pub mod audit;
pub mod backend;
pub mod client;
pub mod degrade;
pub mod prompts;
pub mod tasks;
pub mod types;
mod wire;

#[cfg(test)]
mod mock;

pub use address::{AddressClass, EndpointPolicy, Resolver, SystemResolver, ValidatedEndpoint};
pub use audit::{AuditSink, CallKind, CallOutcome, InferenceEvent, MemoryAuditSink, NullAuditSink};
pub use backend::{Backend, Probe};
pub use client::{LlmClient, LlmConfig};
pub use cyberbrain_core::{EgressPurpose, Error, Result};
pub use degrade::{Degraded, Feature};
pub use types::{ChatMessage, ChatRequest, ChatResponse, LoadedModel, ModelInfo, Role, Usage};
