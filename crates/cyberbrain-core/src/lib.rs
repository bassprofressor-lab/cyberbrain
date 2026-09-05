//! Cyberbrain core: the shared vocabulary every other crate speaks.
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2.
//! See `docs/SPEC.md` §0 for the clean-room boundary this project is built under.

pub mod citation;
pub mod error;
pub mod types;

pub use citation::Citation;
pub use error::{Error, Result};
pub use types::{
    Block, Conflict, EgressPurpose, Embedder, Frontmatter, Hit, Note, NoteId, NoteKind, PiiState,
    RecallResult, Ring,
};
