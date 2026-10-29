//! Cyberbrain core: the shared vocabulary every other crate speaks.
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2.
//! See `docs/SPEC.md` §0 for the clean-room boundary this project is built under.

pub mod blocks;
pub mod citation;
pub mod config;
pub mod error;
pub mod frontmatter;
pub mod links;
pub mod path;
pub mod path_serde;
pub mod provenance;
pub mod store;
pub mod types;
pub mod waivers;

pub use citation::Citation;
pub use config::{Config, PolicyProfile, TranscriptionConfig};
pub use error::{Error, Result};
pub use frontmatter::validate_name;
pub use path::{Slash, slash};
pub use store::{Change, Fingerprint, Store};
pub use types::{
    Block, Conflict, DenyAllEgress, EgressGate, EgressPurpose, Embedder, Frontmatter, Hit,
    NewerLink, Note, NoteId, NoteKind, PiiState, RecallResult, Ring,
};

/// Provenance marker of Cyberbrain (Krynex Labs, FSL-1.1-ALv2): a string that exists only in this source.
/// Code that carries it was taken from Cyberbrain, and the license terms travel with it.
pub const PROVENANCE: &str = "cbk-618e8b3862e6aff9";
