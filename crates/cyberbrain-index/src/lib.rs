//! Cyberbrain index: the SQLite cache over the notes tree (SPEC §5, §7).
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2. Written from
//! `docs/SPEC.md` alone under the clean-room boundary in SPEC §0.
//!
//! What lives here:
//!
//! - the schema and its versioned migrations ([`schema`]),
//! - [`Index`]: open, upsert, delete, lookups, the embedding-profile guard,
//! - hybrid recall: FTS5 BM25 + flat cosine scan, fused with reciprocal rank fusion and
//!   weighted by ring ([`recall`]),
//! - the append-only audit table that `cyberbrain-policy` writes to ([`audit`]).
//!
//! The index is a cache. Every row except the audit log is derivable from the Markdown
//! files, and `tests::rebuild_is_lossless` proves that deleting the database and
//! reindexing gives byte-identical recall output.

#![forbid(unsafe_code)]

pub mod audit;
pub mod index;
pub mod recall;
pub mod schema;
pub mod vectors;

#[cfg(test)]
mod tests;

pub use audit::{AuditEntry, AuditFilter};
pub use index::{
    EmbeddingProfile, Erased, Index, IndexStats, Link, NoteRecord, NoteStamp, UpsertOutcome,
    content_hash,
};
pub use recall::{RRF_K, RecallOptions};
pub use schema::SCHEMA_VERSION;

/// Maps a `rusqlite` failure onto the tree-wide error type. `rusqlite::Error` is foreign to
/// both crates, so a `From` impl is not possible.
pub(crate) trait SqlResultExt<T> {
    fn ix(self) -> cyberbrain_core::Result<T>;
}

impl<T> SqlResultExt<T> for rusqlite::Result<T> {
    fn ix(self) -> cyberbrain_core::Result<T> {
        self.map_err(|e| cyberbrain_core::Error::Index(e.to_string()))
    }
}
