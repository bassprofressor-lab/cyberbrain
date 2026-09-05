//! Cyberbrain embed: static token embeddings in the model2vec format, implemented in-tree
//! (SPEC §6).
//!
//! Original work, copyright 2026 Krynex Labs, licensed FSL-1.1-ALv2. Written from
//! `docs/SPEC.md` alone under the clean-room boundary in §0.
//!
//! # What this crate does
//!
//! A model2vec-format model is two files: a `tokenizer.json` (HuggingFace tokenizers
//! format) and a `model.safetensors` holding one embedding matrix of shape
//! `[vocab, dim]`. The whole forward pass is
//!
//! 1. tokenise the text (no special tokens),
//! 2. look up one row per token id, skipping the unknown token,
//! 3. mean-pool the rows,
//! 4. L2-normalise.
//!
//! There is no transformer. Producing a vector costs a few microseconds.
//!
//! # What this crate never does
//!
//! **No network I/O of any kind.** Model artefacts are loaded from a local path handed in by
//! the caller (the policy crate's registered `ModelDownload` egress path, SPEC §12.1). Every
//! load verifies a blake3 hash of both files against a manifest and refuses on mismatch.
//! There is no "verify later", no "warn and continue" and no default download location.
//!
//! # Degenerate inputs
//!
//! An empty string, or a string whose every token is unknown to the model, has no rows to
//! pool. Dividing by zero there would yield a NaN vector, and a NaN that reaches a cosine
//! comparison silently corrupts every ranking it touches. Instead such inputs yield the
//! **all-zero vector** and [`Embedding::tokens_known`] reports `0`, so a caller can tell the
//! case apart and log it (SPEC §14.5: a branch that declines to act says why). The zero
//! vector has cosine 0 against everything, which ranks it last rather than randomly.
//! [`is_zero`] is the check callers should use before running a semantic search with such a
//! query.

mod artefact;
mod model;
mod pool;
mod weights;

#[cfg(any(test, feature = "synthetic"))]
pub mod synthetic;

#[cfg(test)]
mod tests;

pub use artefact::{ArtefactManifest, ModelPaths, hash_bytes, hash_file};
pub use model::{Embedding, LoadOptions, ModelInfo, StaticEmbedder};
pub use pool::is_zero;

/// The only pooling this crate implements. Part of every profile id, so a future pooling
/// change cannot be confused with vectors produced by this one.
pub const POOLING: &str = "mean";

/// The tensor name a model2vec artefact uses for its embedding matrix.
pub const EMBEDDINGS_TENSOR: &str = "embeddings";
