//! A tiny model2vec-format artefact built on the fly, so tests (here and in other crates
//! behind the `synthetic` feature) never need a real model file or a network.
//!
//! The tokenizer is a lowercase + whitespace WordLevel model: every whitespace-separated
//! word is one token, unknown words map to `[UNK]` at id 0. The matrix is deterministic
//! pseudo-random from a seed, so two builds with the same inputs hash identically.

use crate::artefact::{ArtefactManifest, ModelPaths, hash_bytes};
use cyberbrain_core::{Error, Result};
use safetensors::Dtype;
use safetensors::tensor::TensorView;
use std::path::Path;

/// The unknown token every synthetic model carries at id 0.
pub const UNK: &str = "[UNK]";

/// Writes `tokenizer.json` and `model.safetensors` into `dir` and returns their paths and
/// manifest. `vocab` must not contain `[UNK]` or whitespace; ids are assigned in order
/// starting at 1. Weights are `f32`, shape `[vocab.len() + 1, dim]`.
pub fn write_synthetic_model(
    dir: &Path,
    vocab: &[&str],
    dim: usize,
    seed: u64,
) -> Result<(ModelPaths, ArtefactManifest)> {
    let paths = ModelPaths::in_dir(dir);
    let tokenizer_json = tokenizer_json(vocab)?;
    let weights = weights_bytes(vocab.len() + 1, dim, seed)?;
    write(&paths.tokenizer, tokenizer_json.as_bytes())?;
    write(&paths.weights, &weights)?;
    let manifest = ArtefactManifest {
        weights_blake3: hash_bytes(&weights),
        tokenizer_blake3: hash_bytes(tokenizer_json.as_bytes()),
    };
    Ok((paths, manifest))
}

/// Deterministic pseudo-random weights in `(-1, 1)`, row 0 (the unk row) included so a
/// bug that pools it shows up as a changed vector rather than a panic.
pub fn synthetic_weights(rows: usize, dim: usize, seed: u64) -> Vec<f32> {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    (0..rows * dim)
        .map(|_| {
            // xorshift64*
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let r = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
            ((r >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
        })
        .collect()
}

fn weights_bytes(rows: usize, dim: usize, seed: u64) -> Result<Vec<u8>> {
    let data = synthetic_weights(rows, dim, seed);
    let raw: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
    let view = TensorView::new(Dtype::F32, vec![rows, dim], &raw)
        .map_err(|e| Error::Embed(format!("synthetic tensor: {e}")))?;
    safetensors::serialize([(crate::EMBEDDINGS_TENSOR, view)], None)
        .map_err(|e| Error::Embed(format!("synthetic safetensors: {e}")))
}

fn tokenizer_json(vocab: &[&str]) -> Result<String> {
    let mut entries = vec![format!("{UNK:?}:0")];
    for (i, word) in vocab.iter().enumerate() {
        if *word == UNK || word.chars().any(char::is_whitespace) || word.contains('"') {
            return Err(Error::Embed(format!(
                "synthetic vocab entry {word:?} is not a plain word"
            )));
        }
        entries.push(format!("{word:?}:{}", i + 1));
    }
    Ok(format!(
        concat!(
            "{{\"version\":\"1.0\",\"truncation\":null,\"padding\":null,",
            "\"added_tokens\":[],",
            "\"normalizer\":{{\"type\":\"Lowercase\"}},",
            "\"pre_tokenizer\":{{\"type\":\"Whitespace\"}},",
            "\"post_processor\":null,\"decoder\":null,",
            "\"model\":{{\"type\":\"WordLevel\",\"vocab\":{{{}}},\"unk_token\":{:?}}}}}"
        ),
        entries.join(","),
        UNK
    ))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
