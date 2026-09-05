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
pub mod store;
pub mod types;

pub use citation::Citation;
pub use config::{Config, PolicyProfile};
pub use error::{Error, Result};
pub use frontmatter::validate_name;
pub use path::{Slash, slash};
pub use store::{Change, Fingerprint, Store};
pub use types::{
    Block, Conflict, DenyAllEgress, EgressGate, EgressPurpose, Embedder, Frontmatter, Hit, Note,
    NoteId, NoteKind, PiiState, RecallResult, Ring,
};
