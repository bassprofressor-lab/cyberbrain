//! Hybrid recall (SPEC §7): BM25 candidates from FTS5, cosine candidates from a flat scan,
//! reciprocal rank fusion, ring weighting, top `n`.
//!
//! There is no lexical-only mode. When the semantic half cannot run, recall still answers
//! from the lexical half and the `caveats` say exactly why.

use crate::vectors::{VectorCache, normalize};
use crate::{Index, SqlResultExt};
use cyberbrain_core::{Embedder, Error, Hit, RecallResult, Result, Ring};
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
    /// Restrict to notes of exactly this bereich. A filter, never a ranking input: a
    /// department decides whether a note is *eligible*, not how trustworthy it is.
    pub bereich: Option<String>,
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
            bereich: None,
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

        // 0. The bereich filter, resolved once to the citations it admits. The lexical half
        // filters in SQL; the semantic half has no SQL to join, so it tests membership here.
        let allowed: Option<std::collections::HashSet<String>> = match &opts.bereich {
            None => None,
            Some(b) => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT b.citation FROM blocks b JOIN notes n ON n.id = b.note_id \
                         WHERE n.bereich = ?1",
                    )
                    .ix()?;
                let mut set = std::collections::HashSet::new();
                let rows = stmt.query_map([b], |r| r.get::<_, String>(0)).ix()?;
                for r in rows {
                    set.insert(r.ix()?);
                }
                if set.is_empty() {
                    caveats.push(format!("no note carries bereich {b:?}; no hits are possible"));
                }
                Some(set)
            }
        };

        // 1. Lexical.
        let lexical: Vec<String> = match fts_query(query) {
            None => {
                caveats
                    .push("lexical search skipped: the query contains no searchable terms".into());
                Vec::new()
            }
            Some(q) => self.lexical(&q, opts.k_lex, opts.ring, opts.bereich.as_deref())?,
        };

        // 2. Semantic, behind the profile guard.
        let semantic: Vec<String> = match embedder {
            None => {
                caveats.push(
                    "semantic search skipped: no embedder configured; hits are lexical only".into(),
                );
                Vec::new()
            }
            Some(e) => match self.semantic(
                query,
                e,
                opts.k_sem,
                opts.ring,
                allowed.as_ref(),
                opts.min_cosine,
            ) {
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

        // 4. Ring weight, then recency. The ring is the citation's first component, so no
        // lookup; `updated` needs one, and only for the candidates that survived fusion.
        let stand = self.updated_by_citation(fused.keys().copied())?;
        let now = jiff::Timestamp::now();
        let mut ranked: Vec<(String, f32)> = fused
            .into_iter()
            .map(|(cit, s)| {
                let ring = cit
                    .parse::<cyberbrain_core::Citation>()
                    .map(|c| c.ring)
                    .unwrap_or(Ring::External);
                let recency = stand
                    .get(cit)
                    .map(|u| recency_weight(*u, now))
                    .unwrap_or(1.0);
                (cit.to_string(), s * ring.weight() * recency)
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

    /// `updated` of the note behind each citation, for the recency weight. One statement
    /// for the whole candidate set: a per-hit query would be a hundred round trips.
    fn updated_by_citation<'a>(
        &self,
        citations: impl Iterator<Item = &'a str>,
    ) -> Result<HashMap<String, jiff::Timestamp>> {
        let mut out = HashMap::new();
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT n.updated FROM blocks b JOIN notes n ON n.id = b.note_id \
                 WHERE b.citation = ?1",
            )
            .ix()?;
        for cit in citations {
            let stamp: Option<String> = match stmt.query_row([cit], |r| r.get::<_, String>(0)) {
                Ok(v) => Some(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(Error::Index(format!("updated of {cit}: {e}"))),
            };
            if let Some(s) = stamp
                && let Ok(t) = s.parse::<jiff::Timestamp>()
            {
                out.insert(cit.to_string(), t);
            }
        }
        Ok(out)
    }

    /// Citations in BM25 order, best first. Ties broken by citation for reproducibility.
    fn lexical(
        &self,
        fts: &str,
        k: usize,
        ring: Option<Ring>,
        bereich: Option<&str>,
    ) -> Result<Vec<String>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        // Deliberately the default bm25 weighting, all columns equal. Weighting the name
        // five times was preregistered and measured on 34 labelled questions over a real
        // 1,004-note store: it found one more note inside the top ten and cost one that had
        // been first, so recall@1 fell 0.62 -> 0.59 and MRR stayed flat. The rule said the
        // primary metric had to rise, so it was dropped rather than kept for the anecdote
        // it fixed. A title hit therefore ranks like any other, and a note whose keyword is
        // only in its name can still be outranked by a block that says the word twice.
        // The bereich filter joins rather than post-filters: dropping rows after the LIMIT
        // would let a small department come back empty because the top k all belong to
        // another one.
        let mut out = Vec::new();
        let mut sql = String::from("SELECT f.citation FROM blocks_fts f");
        if bereich.is_some() {
            sql.push_str(
                " JOIN blocks b ON b.citation = f.citation JOIN notes n ON n.id = b.note_id",
            );
        }
        sql.push_str(" WHERE f.blocks_fts MATCH ?1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts.to_string())];
        if let Some(r) = ring {
            sql.push_str(" AND f.ring = ?");
            sql.push_str(&(args.len() + 1).to_string());
            args.push(Box::new(r.as_u8() as i64));
        }
        if let Some(bx) = bereich {
            sql.push_str(" AND n.bereich = ?");
            sql.push_str(&(args.len() + 1).to_string());
            args.push(Box::new(bx.to_string()));
        }
        sql.push_str(" ORDER BY rank, f.citation LIMIT ?");
        sql.push_str(&(args.len() + 1).to_string());
        args.push(Box::new(k as i64));
        let mut stmt = self.conn.prepare_cached(&sql).ix()?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(params.as_slice(), |r| r.get::<_, String>(0))
            .ix()?;
        for r in rows {
            out.push(r.ix()?);
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
        allowed: Option<&std::collections::HashSet<String>>,
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
        let top = cache.top_k(&qv, k, ring, allowed, min_cosine);
        Ok((
            top.into_iter()
                .map(|(_, i)| cache.citations[i].clone())
                .collect(),
            coverage,
        ))
    }
}

/// Half-life of the recency bonus, in days. After this long a note has given up half of
/// whatever freshness advantage it started with.
pub(crate) const RECENCY_HALF_LIFE_DAYS: f32 = 90.0;

/// How far recency may move a score, up or down. Deliberately smaller than the closest
/// gap between two ring weights (1.15 / 1.10 = 1.0455): a fresh note must never outrank a
/// more trusted one on age alone. Age breaks ties *within* a ring; it does not re-rank
/// across rings. See `recency_never_beats_a_ring` in tests.
pub(crate) const RECENCY_AMPLITUDE: f32 = 0.015;

/// A multiplier in `[1 - A, 1 + A]`, decaying by half every [`RECENCY_HALF_LIFE_DAYS`].
/// A note updated right now gets `1 + A`; one from long ago approaches `1 - A`. Notes
/// stamped in the future are treated as current rather than given an unbounded bonus.
pub(crate) fn recency_weight(updated: jiff::Timestamp, now: jiff::Timestamp) -> f32 {
    let age_days = (now.as_second() - updated.as_second()) as f32 / 86_400.0;
    let decay = if age_days <= 0.0 {
        1.0
    } else {
        0.5f32.powf(age_days / RECENCY_HALF_LIFE_DAYS)
    };
    1.0 - RECENCY_AMPLITUDE + 2.0 * RECENCY_AMPLITUDE * decay
}

/// Why the semantic half did not run. Always becomes a caveat, never an error.
struct SemanticSkipped(String);
