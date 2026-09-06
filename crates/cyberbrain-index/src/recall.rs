//! Hybrid recall (SPEC §7): BM25 candidates from FTS5, cosine candidates from a flat scan,
//! reciprocal rank fusion, ring weighting, top `n`.
//!
//! There is no lexical-only mode. When the semantic half cannot run, recall still answers
//! from the lexical half and the `caveats` say exactly why.

use crate::vectors::{VectorCache, normalize};
use crate::{Index, SqlResultExt};
use cyberbrain_core::{Embedder, Error, Hit, RecallResult, Result, Ring};
use rusqlite::params;
use std::collections::HashMap;

/// The constant in `1 / (RRF_K + rank)`.
pub const RRF_K: f32 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub struct RecallOptions {
    /// Hits returned.
    pub n: usize,
    /// Lexical candidates considered.
    pub k_lex: usize,
    /// Semantic candidates considered.
    pub k_sem: usize,
    /// Restrict to exactly this ring.
    pub ring: Option<Ring>,
    /// A block must score strictly above this cosine to become a semantic candidate. The
    /// default `0.0` only drops blocks with no similarity evidence at all; RRF would
    /// otherwise hand a rank, and thus a score, to the top `k_sem` of an unrelated corpus.
    pub min_cosine: f32,
}

impl Default for RecallOptions {
    fn default() -> Self {
        Self {
            n: 8,
            k_lex: 50,
            k_sem: 50,
            ring: None,
            min_cosine: 0.0,
        }
    }
}

/// Turn free text into an FTS5 MATCH expression that cannot fail to parse: each term is
/// quoted and the terms are ORed, so a typo in one term does not empty the result and a
/// stray `-`, `:` or `"` in the query is never read as syntax. BM25 still ranks the
/// blocks that match more terms higher. `None` when nothing searchable is left.
///
/// Terms of four characters or more are prefix terms (`"postgres"*`). The tokenizer is
/// `unicode61`, which does no stemming: without this, `postgres` misses `PostgreSQL` and
/// `Vektor` misses `Vektoren`, and the reader has to guess the exact form somebody wrote
/// months ago. Three characters and under stay exact, because `"in"*` matches half the
/// store and buys nothing. A prefix term also covers the exact word, so nothing that
/// matched before stops matching.
pub fn fts_query(query: &str) -> Option<String> {
    const MAX_TERMS: usize = 32;
    /// Shorter terms are prefixes of too much to be worth the candidates they drag in.
    const PREFIX_FROM: usize = 4;
    let terms: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .take(MAX_TERMS)
        .map(|t| {
            if t.chars().count() >= PREFIX_FROM {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

impl Index {
    /// Hybrid recall. `embedder` supplies the query vector; `None` means lexical only,
    /// which is recorded in `caveats` rather than silently accepted.
    pub fn recall(
        &self,
        query: &str,
        embedder: Option<&dyn Embedder>,
        opts: &RecallOptions,
    ) -> Result<RecallResult> {
        let mut caveats: Vec<String> = Vec::new();

        // 1. Lexical.
        let lexical: Vec<String> = match fts_query(query) {
            None => {
                caveats
                    .push("lexical search skipped: the query contains no searchable terms".into());
                Vec::new()
            }
            Some(q) => self.lexical(&q, opts.k_lex, opts.ring)?,
        };

        // 2. Semantic, behind the profile guard.
        let semantic: Vec<String> = match embedder {
            None => {
                caveats.push(
                    "semantic search skipped: no embedder configured; hits are lexical only".into(),
                );
                Vec::new()
            }
            Some(e) => match self.semantic(query, e, opts.k_sem, opts.ring, opts.min_cosine) {
                Ok((hits, coverage)) => {
                    if let Some(c) = coverage {
                        caveats.push(c);
                    }
                    hits
                }
                Err(SemanticSkipped(reason)) => {
                    caveats.push(reason);
                    Vec::new()
                }
            },
        };

        // 3. Reciprocal rank fusion, 1-based ranks.
        let mut fused: HashMap<&str, f32> = HashMap::new();
        for list in [&lexical, &semantic] {
            for (rank, cit) in list.iter().enumerate() {
                *fused.entry(cit.as_str()).or_insert(0.0) += 1.0 / (RRF_K + (rank as f32 + 1.0));
            }
        }

        // 4. Ring weight. The ring is the citation's first component, so no lookup.
        let mut ranked: Vec<(String, f32)> = fused
            .into_iter()
            .map(|(cit, s)| {
                let ring = cit
                    .parse::<cyberbrain_core::Citation>()
                    .map(|c| c.ring)
                    .unwrap_or(Ring::External);
                (cit.to_string(), s * ring.weight())
            })
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(opts.n);

        // 5. Materialise.
        let mut hits = Vec::with_capacity(ranked.len());
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT n.id, n.name, n.ring, b.text FROM blocks b JOIN notes n ON n.id = b.note_id \
                 WHERE b.citation = ?1",
            )
            .ix()?;
        for (cit, score) in ranked {
            let (id, name, ring, text): (String, String, i64, String) = stmt
                .query_row([&cit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .map_err(|e| Error::Index(format!("hit {cit} vanished during recall: {e}")))?;
            hits.push(Hit {
                citation: cit,
                note_id: cyberbrain_core::NoteId::from_string(&id)
                    .map_err(|e| Error::Index(format!("stored note id {id:?}: {e}")))?,
                note_name: name,
                ring: Ring::try_from(ring as u8)?,
                score,
                text,
            });
        }

        Ok(RecallResult {
            hits,
            conflicts: Vec::new(),
            caveats,
        })
    }

    /// Citations in BM25 order, best first. Ties broken by citation for reproducibility.
    fn lexical(&self, fts: &str, k: usize, ring: Option<Ring>) -> Result<Vec<String>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        match ring {
            None => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT citation FROM blocks_fts WHERE blocks_fts MATCH ?1 \
                         ORDER BY rank, citation LIMIT ?2",
                    )
                    .ix()?;
                let rows = stmt
                    .query_map(params![fts, k as i64], |r| r.get::<_, String>(0))
                    .ix()?;
                for r in rows {
                    out.push(r.ix()?);
                }
            }
            Some(ring) => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT citation FROM blocks_fts WHERE blocks_fts MATCH ?1 AND ring = ?2 \
                         ORDER BY rank, citation LIMIT ?3",
                    )
                    .ix()?;
                let rows = stmt
                    .query_map(params![fts, ring.as_u8() as i64, k as i64], |r| {
                        r.get::<_, String>(0)
                    })
                    .ix()?;
                for r in rows {
                    out.push(r.ix()?);
                }
            }
        }
        Ok(out)
    }

    /// Citations by cosine, best first, plus a coverage caveat when some blocks have no
    /// vector. `Err` carries the reason the semantic half was skipped; it is never fatal.
    fn semantic(
        &self,
        query: &str,
        embedder: &dyn Embedder,
        k: usize,
        ring: Option<Ring>,
        min_cosine: f32,
    ) -> std::result::Result<(Vec<String>, Option<String>), SemanticSkipped> {
        let fatal = |e: Error| SemanticSkipped(format!("semantic search failed: {e}"));
        if let Err(e) = self.check_embedder(embedder) {
            return Err(SemanticSkipped(format!("semantic search disabled: {e}")));
        }
        let Some(profile) = self.embedding_profile().map_err(fatal)? else {
            return Err(SemanticSkipped(
                "semantic search disabled: the index records no embedding profile and holds no \
                 vectors; run `cyberbrain scan`"
                    .into(),
            ));
        };
        if k == 0 {
            return Ok((Vec::new(), None));
        }

        let generation = self.generation().map_err(fatal)?;
        let mut cache = self.cache.borrow_mut();
        if cache.as_ref().map(|c| c.generation) != Some(generation) {
            *cache = Some(VectorCache::load(&self.conn, generation, profile.dim).map_err(fatal)?);
        }
        let cache = cache.as_ref().expect("just loaded");

        let block_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM blocks", [], |r| r.get(0))
            .map_err(|e| SemanticSkipped(format!("semantic search failed: {e}")))?;
        let coverage = if cache.len() < block_count as usize {
            Some(format!(
                "semantic search covered {} of {block_count} blocks; the rest have no vector",
                cache.len()
            ))
        } else {
            None
        };
        if cache.len() == 0 {
            return Ok((Vec::new(), coverage));
        }

        let mut qv = embedder
            .embed(&[query])
            .map_err(|e| {
                SemanticSkipped(format!(
                    "semantic search skipped: embedding the query failed: {e}"
                ))
            })?
            .into_iter()
            .next()
            .ok_or_else(|| {
                SemanticSkipped("semantic search skipped: embedder returned no vector".into())
            })?;
        if qv.len() != cache.dim {
            return Err(SemanticSkipped(format!(
                "semantic search disabled: query vector has dim {} but stored vectors have {}",
                qv.len(),
                cache.dim
            )));
        }
        if !normalize(&mut qv) {
            return Err(SemanticSkipped(
                "semantic search skipped: the query embedded to a zero vector".into(),
            ));
        }
        let top = cache.top_k(&qv, k, ring, min_cosine);
        Ok((
            top.into_iter()
                .map(|(_, i)| cache.citations[i].clone())
                .collect(),
            coverage,
        ))
    }
}

/// Why the semantic half did not run. Always becomes a caveat, never an error.
struct SemanticSkipped(String);
