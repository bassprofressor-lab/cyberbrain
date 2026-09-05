//! Local model artefacts and their verification (SPEC §6: "content-addressed by hash,
//! verified on every load; a hash mismatch is a hard failure").
//!
//! Nothing in this module can reach the network. It takes paths and returns bytes.

use cyberbrain_core::{Error, Result, Slash};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Length of a blake3 digest rendered as lowercase hex.
const HEX_LEN: usize = 64;

/// Where the two files of a model2vec-format model live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    /// `model.safetensors`: one tensor named `embeddings`, shape `[vocab, dim]`.
    pub weights: PathBuf,
    /// `tokenizer.json` in HuggingFace tokenizers format.
    pub tokenizer: PathBuf,
}

impl ModelPaths {
    /// The conventional layout: `<dir>/model.safetensors` and `<dir>/tokenizer.json`.
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            weights: dir.join("model.safetensors"),
            tokenizer: dir.join("tokenizer.json"),
        }
    }
}

/// The expected blake3 digests of both files, lowercase hex. Produced once by whoever
/// obtained the artefact (the policy crate, after its registered download) and stored in
/// configuration; checked on every load.
///
/// Both files are covered because both change the meaning of a vector: a different
/// tokenizer maps the same text to different rows of the same matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtefactManifest {
    pub weights_blake3: String,
    pub tokenizer_blake3: String,
}

impl ArtefactManifest {
    /// Rejects anything that is not exactly 64 lowercase hex characters per digest. A
    /// manifest that cannot match anything is a configuration error, not a load failure.
    pub fn validate(&self) -> Result<()> {
        for (what, hex) in [
            ("weights_blake3", &self.weights_blake3),
            ("tokenizer_blake3", &self.tokenizer_blake3),
        ] {
            if hex.len() != HEX_LEN
                || !hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Config(format!(
                    "model manifest: {what} must be {HEX_LEN} lowercase hex characters, \
                     got {:?}",
                    hex
                )));
            }
        }
        Ok(())
    }
}

/// blake3 of a byte slice, lowercase hex.
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// blake3 of a file's contents, lowercase hex. This is what the policy crate calls after a
/// download to fill an [`ArtefactManifest`].
pub fn hash_file(path: impl AsRef<Path>) -> Result<String> {
    let bytes = read(path.as_ref())?;
    Ok(hash_bytes(&bytes))
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Reads a file and verifies its digest. Returns the bytes only when they match, so the
/// bytes that were hashed are the bytes that get parsed: no window between check and use.
pub(crate) fn read_verified(path: &Path, expected_hex: &str, what: &str) -> Result<Vec<u8>> {
    let bytes = read(path)?;
    let actual = hash_bytes(&bytes);
    if actual != expected_hex {
        return Err(Error::Embed(format!(
            "{what} artefact {} does not match its manifest: expected blake3 {expected_hex}, \
             file has {actual}; refusing to load (SPEC §6: a hash mismatch is a hard failure)",
            Slash(path)
        )));
    }
    Ok(bytes)
}
