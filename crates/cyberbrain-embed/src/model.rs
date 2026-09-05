//! The static embedder: tokenizer + embedding matrix, mean pooling, L2 normalisation.

use crate::POOLING;
use crate::artefact::{ArtefactManifest, ModelPaths, read_verified};
use crate::pool;
use crate::weights::{Matrix, load_matrix};
use cyberbrain_core::{Embedder, Error, Result};
use rayon::prelude::*;
use serde::Serialize;
use tokenizers::Tokenizer;
use tokenizers::models::ModelWrapper;

/// Batches at least this large are split across rayon's thread pool. Below it the
/// per-item work (a few microseconds) is smaller than the scheduling overhead, and the
/// hook path embeds exactly one query at a time.
const PARALLEL_THRESHOLD: usize = 64;

/// Unknown-token names probed when the tokenizer model does not expose its own.
const UNK_CANDIDATES: [&str; 4] = ["[UNK]", "<unk>", "<|unk|>", "[unk]"];

/// Knobs for loading. `Default` is what production uses.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Keep at most this many token ids per input before pooling. `None` pools every
    /// token. Truncation is applied to the id list after tokenisation, never by the
    /// tokenizer's own truncation setting, which is disabled on load.
    pub max_tokens: Option<usize>,
    /// Override the unknown-token string. Normally derived from the tokenizer model.
    pub unk_token: Option<String>,
}

/// One embedded input. The vector is L2-normalised, or all zero when nothing was pooled.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub vector: Vec<f32>,
    /// Token ids the tokenizer produced for the input (after `max_tokens`).
    pub tokens_seen: usize,
    /// Of those, how many had a row in the matrix. `0` means the vector is all zero.
    pub tokens_known: usize,
}

impl Embedding {
    /// Nothing was pooled: empty input, or every token unknown. The vector is all zero.
    pub fn is_empty(&self) -> bool {
        self.tokens_known == 0
    }
}

/// Technical facts about the loaded model, for `status` and the model card (SPEC §12.7).
/// Source and licence are not known here; the policy crate's registry carries those.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    pub profile_id: String,
    pub dim: usize,
    pub vocab_rows: usize,
    pub weights_dtype: &'static str,
    pub pooling: &'static str,
    pub weights_blake3: String,
    pub tokenizer_blake3: String,
    pub unk_id: Option<u32>,
    pub max_tokens: Option<usize>,
}

/// A loaded model2vec-format model. Cheap to share behind an `Arc`; `embed` takes `&self`.
pub struct StaticEmbedder {
    tokenizer: Tokenizer,
    matrix: Matrix,
    unk_id: Option<u32>,
    max_tokens: Option<usize>,
    manifest: ArtefactManifest,
    profile_id: String,
}

impl std::fmt::Debug for StaticEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticEmbedder")
            .field("profile_id", &self.profile_id)
            .field("dim", &self.matrix.dim)
            .field("vocab_rows", &self.matrix.rows)
            .field("dtype", &self.matrix.dtype)
            .field("unk_id", &self.unk_id)
            .finish_non_exhaustive()
    }
}

impl StaticEmbedder {
    /// Loads and verifies a model from local files. Fails, never warns, when either file's
    /// blake3 digest differs from the manifest, when the files do not parse, when the
    /// matrix is not `[vocab, dim]`, contains a non-finite value, or has fewer rows than
    /// the tokenizer has ids.
    pub fn load(paths: &ModelPaths, manifest: &ArtefactManifest) -> Result<Self> {
        Self::load_with(paths, manifest, LoadOptions::default())
    }

    pub fn load_with(
        paths: &ModelPaths,
        manifest: &ArtefactManifest,
        opts: LoadOptions,
    ) -> Result<Self> {
        manifest.validate()?;

        let tok_bytes = read_verified(&paths.tokenizer, &manifest.tokenizer_blake3, "tokenizer")?;
        let w_bytes = read_verified(&paths.weights, &manifest.weights_blake3, "weights")?;

        let mut tokenizer = Tokenizer::from_bytes(&tok_bytes).map_err(|e| {
            Error::Embed(format!(
                "tokenizer {} is not a valid tokenizers file: {e}",
                paths.tokenizer.display()
            ))
        })?;
        // The artefact may carry the source model's padding/truncation. Neither belongs in
        // a mean-pooled lookup: padding would pool the pad row, truncation would drop text
        // without a trace. `max_tokens` is applied explicitly instead.
        tokenizer.with_padding(None);
        tokenizer
            .with_truncation(None)
            .map_err(|e| Error::Embed(format!("cannot disable tokenizer truncation: {e}")))?;

        let matrix = load_matrix(&w_bytes)?;

        let vocab = tokenizer.get_vocab_size(true);
        if vocab > matrix.rows {
            return Err(Error::Embed(format!(
                "tokenizer knows {vocab} ids but the embedding matrix has only {} rows; the \
                 two files do not belong to the same model",
                matrix.rows
            )));
        }

        let unk_id = resolve_unk(&tokenizer, opts.unk_token.as_deref());

        // Everything that changes the meaning of a stored vector goes in here: both file
        // digests (weights and tokenizer), the dimension and the pooling name.
        let profile_id = {
            let mut h = blake3::Hasher::new();
            h.update(manifest.weights_blake3.as_bytes());
            h.update(manifest.tokenizer_blake3.as_bytes());
            let short = &h.finalize().to_hex()[..16];
            format!("m2v-{POOLING}-d{}-{short}", matrix.dim)
        };

        Ok(Self {
            tokenizer,
            matrix,
            unk_id,
            max_tokens: opts.max_tokens,
            manifest: manifest.clone(),
            profile_id,
        })
    }

    /// Embeds one input. Errors only on a tokenizer failure, which no ordinary text causes.
    pub fn embed_one(&self, text: &str) -> Result<Embedding> {
        let enc = self
            .tokenizer
            .encode_fast(text, false)
            .map_err(|e| Error::Embed(format!("tokenizer failed on input: {e}")))?;
        let mut ids = enc.get_ids();
        if let Some(max) = self.max_tokens
            && ids.len() > max
        {
            ids = &ids[..max];
        }
        Ok(self.pool_ids(ids))
    }

    /// Embeds a batch, in input order, splitting large batches across rayon's pool.
    pub fn embed_all(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        self.embed_all_impl(texts, texts.len() >= PARALLEL_THRESHOLD)
    }

    pub(crate) fn embed_all_impl(&self, texts: &[&str], parallel: bool) -> Result<Vec<Embedding>> {
        if parallel {
            texts.par_iter().map(|t| self.embed_one(t)).collect()
        } else {
            texts.iter().map(|t| self.embed_one(t)).collect()
        }
    }

    fn pool_ids(&self, ids: &[u32]) -> Embedding {
        let dim = self.matrix.dim;
        let rows = self.matrix.rows;
        let data: &[f32] = &self.matrix.data;
        let mut vector = vec![0.0f32; dim];
        let known_rows = ids.iter().filter_map(|&id| {
            let i = id as usize;
            // An id at or past the last row cannot happen after the load-time vocab check,
            // but an out-of-bounds slice would panic; treat it as unknown instead.
            if Some(id) == self.unk_id || i >= rows {
                None
            } else {
                Some(&data[i * dim..(i + 1) * dim])
            }
        });
        let tokens_known = pool::mean_pool_normalise(&mut vector, known_rows);
        Embedding {
            vector,
            tokens_seen: ids.len(),
            tokens_known,
        }
    }

    pub fn vocab_rows(&self) -> usize {
        self.matrix.rows
    }

    pub fn unk_id(&self) -> Option<u32> {
        self.unk_id
    }

    pub fn manifest(&self) -> &ArtefactManifest {
        &self.manifest
    }

    pub fn info(&self) -> ModelInfo {
        ModelInfo {
            profile_id: self.profile_id.clone(),
            dim: self.matrix.dim,
            vocab_rows: self.matrix.rows,
            weights_dtype: self.matrix.dtype,
            pooling: POOLING,
            weights_blake3: self.manifest.weights_blake3.clone(),
            tokenizer_blake3: self.manifest.tokenizer_blake3.clone(),
            unk_id: self.unk_id,
            max_tokens: self.max_tokens,
        }
    }
}

/// Finds the id of the unknown token so it can be skipped in pooling (model2vec drops it
/// rather than averaging a meaningless row into every vector that has a typo).
fn resolve_unk(tokenizer: &Tokenizer, override_name: Option<&str>) -> Option<u32> {
    if let Some(name) = override_name {
        return tokenizer.token_to_id(name);
    }
    let from_model: Option<String> = match tokenizer.get_model() {
        ModelWrapper::WordPiece(m) => Some(m.unk_token.clone()),
        ModelWrapper::WordLevel(m) => Some(m.unk_token.clone()),
        ModelWrapper::BPE(m) => m.unk_token.clone(),
        // Unigram keeps its unk id private; fall through to the name probe.
        ModelWrapper::Unigram(_) => None,
    };
    if let Some(id) = from_model.and_then(|n| tokenizer.token_to_id(&n)) {
        return Some(id);
    }
    UNK_CANDIDATES.iter().find_map(|n| tokenizer.token_to_id(n))
}

impl Embedder for StaticEmbedder {
    fn dim(&self) -> usize {
        self.matrix.dim
    }

    fn profile_id(&self) -> &str {
        &self.profile_id
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(self
            .embed_all(texts)?
            .into_iter()
            .map(|e| e.vector)
            .collect())
    }
}
