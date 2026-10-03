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
    /// A client's limit (2026-10-03): only notes whose bereich is one of these. Applied on
    /// top of `bereich`, in the same SQL, and a note with no bereich is outside every limit.
    /// `Some(vec![])` admits nothing.
    pub scope: Option<Vec<String>>,
    /// A block must score strictly above this cosine to become a semantic candidate. The
    /// default `0.0` only drops blocks with no similarity evidence at all; RRF would
    /// otherwise hand a rank, and thus a score, to the top `k_sem` of an unrelated corpus.
    pub min_cosine: f32,
    /// Leave rings 0 and 1 out (unless `ring` asks for one of them). They are injected
    /// whole into every session, so a hit from them is a second copy of what the reader
    /// already holds. A block of another ring whose text is the same as a resident block's
    /// is left out with it.
    pub skip_resident: bool,
    /// The moment validity is judged at: `invalid_at`, `valid_from` and supersession are
    /// read against it. `None` is now. Set, it answers "what held on that day"; notes are
    /// not filtered by when they were written, since a note from September may well say
    /// what held in August.
    pub at: Option<jiff::Timestamp>,
}

impl Default for RecallOptions {
    fn default() -> Self {
        Self {
            n: 8,
            k_lex: 50,
            k_sem: 50,
            ring: None,
            bereich: None,
            scope: None,
            min_cosine: 0.0,
            skip_resident: false,
            at: None,
        }
    }
}

/// `AND n.bereich = ?` for a bereich filter, `AND n.bereich IN (…)` for a client's scope,
/// numbered after the arguments already bound. An empty scope admits nothing.
fn bereich_clause(
    sql: &mut String,
    args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    bereich: Option<&str>,
    scope: Option<&[String]>,
) {
    if let Some(bx) = bereich {
        sql.push_str(" AND n.bereich = ?");
        sql.push_str(&(args.len() + 1).to_string());
        args.push(Box::new(bx.to_string()));
    }
    if let Some(sc) = scope {
        if sc.is_empty() {
            sql.push_str(" AND 0");
            return;
        }
        sql.push_str(" AND n.bereich IN (");
        for (i, b) in sc.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            sql.push_str(&(args.len() + 1).to_string());
            args.push(Box::new(b.clone()));
        }
        sql.push(')');
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
        let allowed: Option<std::collections::HashSet<String>> =
            if opts.bereich.is_none() && opts.scope.is_none() {
                None
            } else {
                let mut sql = String::from(
                    "SELECT b.citation FROM blocks b JOIN notes n ON n.id = b.note_id WHERE 1=1",
                );
                let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                bereich_clause(
                    &mut sql,
                    &mut args,
                    opts.bereich.as_deref(),
                    opts.scope.as_deref(),
                );
                let mut stmt = self.conn.prepare_cached(&sql).ix()?;
                let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
                let mut set = std::collections::HashSet::new();
                let rows = stmt
                    .query_map(params.as_slice(), |r| r.get::<_, String>(0))
                    .ix()?;
                for r in rows {
                    set.insert(r.ix()?);
                }
                if set.is_empty() {
                    caveats.push(match &opts.bereich {
                        Some(b) => format!("no note carries bereich {b:?}; no hits are possible"),
                        None => "no note lies inside this client's bereiche; no hits are possible"
                            .to_string(),
                    });
                }
                Some(set)
            };

        // 1. Lexical.
        let lexical: Vec<String> = match fts_query(query) {
            None => {
                caveats
                    .push("lexical search skipped: the query contains no searchable terms".into());
                Vec::new()
            }
            Some(q) => self.lexical(
                &q,
                opts.k_lex,
                opts.ring,
                opts.bereich.as_deref(),
                opts.scope.as_deref(),
            )?,
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

        // 4. Ring weight, then recency, then whether the note still holds. The ring is the
        // citation's first component, so no lookup; `updated`, the note name and its
        // validity need one, and only for the candidates that survived fusion.
        let stand = self.updated_by_citation(fused.keys().copied())?;
        let now = jiff::Timestamp::now();
        let at = opts.at.unwrap_or(now);
        if let Some(a) = opts.at {
            caveats.push(format!(
                "validity judged as of {a}: `invalid_at`, `valid_from` and supersession are \
                 read against that moment; notes written later are not left out"
            ));
        }
        let replaced = self.superseded_at(at)?;
        let mut outdated: HashMap<String, Outdated> = HashMap::new();
        let mut ranked: Vec<(String, f32)> = fused
            .into_iter()
            .map(|(cit, s)| {
                let ring = cit
                    .parse::<cyberbrain_core::Citation>()
                    .map(|c| c.ring)
                    .unwrap_or(Ring::External);
                let (recency, stale) = match stand.get(cit) {
                    Some(t) => {
                        let o = Outdated {
                            superseded_by: replaced.get(&t.name).cloned(),
                            valid_from: t.valid_from.filter(|f| *f > at),
                            invalid_at: t.invalid_at.filter(|i| *i <= at),
                        };
                        // One weight however many reasons: a note that is both replaced and
                        // expired is not twice as wrong, and ×0.25 would bury the history
                        // somebody may have asked for.
                        let w = if o.any() { OUTDATED_WEIGHT } else { 1.0 };
                        if o.any() {
                            outdated.insert(cit.to_string(), o);
                        }
                        (t.updated.map(|u| recency_weight(u, now)).unwrap_or(1.0), w)
                    }
                    None => (1.0, 1.0),
                };
                (cit.to_string(), s * ring.weight() * recency * stale)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // 5. Materialise every candidate: collapsing duplicates needs the text of each, and
        // the members of one group can sit anywhere in the order. Candidates are at most
        // k_lex + k_sem, each a lookup by a unique key.
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT n.id, n.name, n.ring, b.text, n.updated, n.tags FROM blocks b \
                 JOIN notes n ON n.id = b.note_id WHERE b.citation = ?1",
            )
            .ix()?;
        let mut cands: Vec<Candidate> = Vec::with_capacity(ranked.len());
        for (cit, score) in ranked {
            let o = outdated.remove(&cit).unwrap_or_default();
            let (id, name, ring, text, updated, tags): (
                String,
                String,
                i64,
                String,
                String,
                String,
            ) = stmt
                .query_row([&cit], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })
                .map_err(|e| Error::Index(format!("hit {cit} vanished during recall: {e}")))?;
            let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
            cands.push(Candidate {
                key: dedup_key(&text),
                hit: Hit {
                    citation: cit,
                    note_id: cyberbrain_core::NoteId::from_string(&id)
                        .map_err(|e| Error::Index(format!("stored note id {id:?}: {e}")))?,
                    ring: Ring::try_from(ring as u8)?,
                    updated: updated.parse().ok(),
                    superseded_by: o.superseded_by,
                    valid_from: o.valid_from,
                    invalid_at: o.invalid_at,
                    newer_links: Vec::new(),
                    untrusted: cyberbrain_core::provenance::untrusted_source(&tags),
                    note_name: name,
                    score,
                    text,
                },
                updated: updated.parse().ok(),
            });
        }

        // 6. One hit per text, then the resident rings out, then the top `n`.
        let skip_resident = opts.skip_resident && opts.ring.is_none();
        let mut hits = Vec::with_capacity(opts.n);
        let mut resident_left_out = 0usize;
        for group in collapse_duplicates(cands) {
            if hits.len() == opts.n {
                break;
            }
            if skip_resident && group.ring.as_u8() <= 1 {
                resident_left_out += 1;
                continue;
            }
            hits.push(group);
        }
        // Not for a client with a scope: the names of linking notes outside it would leak.
        if opts.scope.is_none() {
            for h in &mut hits {
                h.newer_links = self.newer_links_to(h)?;
            }
        }
        if resident_left_out > 0 {
            caveats.push(format!(
                "{resident_left_out} hit(s) from rings 0/1 left out: those rings are in the \
                 session context already; ask with --ring 0 or --ring 1 to search them"
            ));
        }

        Ok(RecallResult {
            hits,
            conflicts: Vec::new(),
            caveats,
        })
    }

    /// Notes updated after `hit`'s that link to its note, newest first, at most three.
    /// Only for the hits returned, so it costs `n` indexed lookups and nothing per candidate.
    fn newer_links_to(&self, hit: &Hit) -> Result<Vec<cyberbrain_core::NewerLink>> {
        let Some(since) = hit.updated else {
            return Ok(Vec::new());
        };
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT n.name, n.ring, n.updated FROM links l JOIN notes n ON n.id = l.from_note \
                 WHERE l.resolved_note_id = ?1 AND n.id != ?1",
            )
            .ix()?;
        let rows = stmt
            .query_map([hit.note_id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .ix()?;
        let mut out = Vec::new();
        for row in rows {
            let (name, ring, updated) = row.ix()?;
            let Ok(updated) = updated.parse::<jiff::Timestamp>() else {
                continue;
            };
            if updated <= since {
                continue;
            }
            out.push(cyberbrain_core::NewerLink {
                name,
                ring: Ring::try_from(ring as u8)?,
                updated,
            });
        }
        out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.name.cmp(&b.name)));
        out.truncate(3);
        Ok(out)
    }

    /// `updated`, name and validity of the note behind each citation, for the recency
    /// weight, supersession and validity. One prepared statement for the whole candidate set.
    fn updated_by_citation<'a>(
        &self,
        citations: impl Iterator<Item = &'a str>,
    ) -> Result<HashMap<String, NoteTimes>> {
        let mut out = HashMap::new();
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT n.updated, n.name, n.valid_from, n.invalid_at FROM blocks b \
                 JOIN notes n ON n.id = b.note_id WHERE b.citation = ?1",
            )
            .ix()?;
        let moment = |s: Option<String>| s.and_then(|s| s.parse::<jiff::Timestamp>().ok());
        for cit in citations {
            match stmt.query_row([cit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            }) {
                Ok((stamp, name, from, until)) => {
                    out.insert(
                        cit.to_string(),
                        NoteTimes {
                            updated: stamp.parse().ok(),
                            name,
                            valid_from: moment(from),
                            invalid_at: moment(until),
                        },
                    );
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(e) => return Err(Error::Index(format!("updated of {cit}: {e}"))),
            }
        }
        Ok(out)
    }

    /// Replaced note name -> the name of the note that replaces it, from both sides:
    /// `superseded_by` on the old note and `supersedes` on the new one. Where two notes
    /// claim the same one, the first by name wins, so the answer does not depend on order.
    pub fn superseded(&self) -> Result<HashMap<String, String>> {
        self.superseded_at(jiff::Timestamp::now())
    }

    /// As [`superseded`](Self::superseded), as of `at`: a replacement counts from the
    /// successor's `valid_from`, or, without one, from when the successor was written. A
    /// successor the index does not hold counts always, as it did before validity existed.
    pub fn superseded_at(&self, at: jiff::Timestamp) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT p.old, p.new, s.valid_from, s.created FROM ( \
                   SELECT name AS old, superseded_by AS new FROM notes \
                   WHERE superseded_by IS NOT NULL \
                   UNION ALL \
                   SELECT j.value, n.name FROM notes n, json_each(n.supersedes) j \
                   WHERE n.supersedes <> '[]' \
                 ) p LEFT JOIN notes s ON s.name = p.new \
                 ORDER BY 1, 2",
            )
            .ix()?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .ix()?;
        let mut out = HashMap::new();
        for r in rows {
            let (old, new, from, created) = r.ix()?;
            let since = from
                .or(created)
                .and_then(|s| s.parse::<jiff::Timestamp>().ok());
            if old != new && since.is_none_or(|t| t <= at) {
                out.entry(old).or_insert(new);
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
        scope: Option<&[String]>,
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
        if bereich.is_some() || scope.is_some() {
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
        bereich_clause(&mut sql, &mut args, bereich, scope);
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

/// When the note behind a candidate was written, and between which moments it holds.
struct NoteTimes {
    updated: Option<jiff::Timestamp>,
    name: String,
    valid_from: Option<jiff::Timestamp>,
    invalid_at: Option<jiff::Timestamp>,
}

/// Why a candidate no longer (or not yet) holds at the moment recall was asked about.
#[derive(Default)]
struct Outdated {
    superseded_by: Option<String>,
    valid_from: Option<jiff::Timestamp>,
    invalid_at: Option<jiff::Timestamp>,
}

impl Outdated {
    fn any(&self) -> bool {
        self.superseded_by.is_some() || self.valid_from.is_some() || self.invalid_at.is_some()
    }
}

/// What the score of a note that does not hold at the moment asked about is multiplied by:
/// replaced, expired (`invalid_at` passed) or not yet in force (`valid_from` ahead). The
/// same factor as supersession, and applied once whichever reasons apply.
pub const OUTDATED_WEIGHT: f32 = SUPERSEDED_WEIGHT;

/// What a replaced note's score is multiplied by. Large on purpose: fused scores sit in a
/// band a few per cent wide, so anything gentler would leave the old note where it was.
/// It is not removed — the history can be what was asked for — only ranked below and
/// marked `superseded_by`.
pub const SUPERSEDED_WEIGHT: f32 = 0.5;

/// Below this many characters (after normalising) two equal blocks are not merged: a
/// short line such as "Status: erledigt." says different things in different notes, and
/// the note name next to it is the information. The measurement that motivated merging
/// (19 % of the orderflow store's blocks have a twin) counted from the same length.
pub const DEDUP_MIN_CHARS: usize = 80;

/// The identity of a block's text for duplicate detection: blake3 over the text with
/// runs of whitespace collapsed and letters lower-cased, so a re-wrapped or re-cased copy
/// is still a copy. `None` for text too short to merge (see [`DEDUP_MIN_CHARS`]).
pub fn dedup_key(text: &str) -> Option<[u8; 32]> {
    let norm = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (norm.chars().count() >= DEDUP_MIN_CHARS).then(|| *blake3::hash(norm.as_bytes()).as_bytes())
}

struct Candidate {
    key: Option<[u8; 32]>,
    hit: Hit,
    updated: Option<jiff::Timestamp>,
}

/// Candidates in rank order in, one hit per distinct text out, still in rank order.
///
/// A group takes the place of its best-ranked member and that member's score, so the
/// order of everything that has no twin is exactly what it was. The block shown is the
/// most trusted copy: the lowest ring, then the most recently updated note, then the best
/// rank. Which copy was shown does not move a group, only which note it is attributed to.
fn collapse_duplicates(cands: Vec<Candidate>) -> Vec<Hit> {
    // (position of the best-ranked member, representative candidate index)
    let mut groups: Vec<(f32, usize)> = Vec::with_capacity(cands.len());
    let mut group_of_key: HashMap<[u8; 32], usize> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        match c.key {
            Some(k) if group_of_key.contains_key(&k) => {
                let g = group_of_key[&k];
                let rep = &cands[groups[g].1];
                let better = (c.hit.ring.as_u8(), std::cmp::Reverse(c.updated))
                    < (rep.hit.ring.as_u8(), std::cmp::Reverse(rep.updated));
                if better {
                    groups[g].1 = i;
                }
            }
            Some(k) => {
                group_of_key.insert(k, groups.len());
                groups.push((c.hit.score, i));
            }
            None => groups.push((c.hit.score, i)),
        }
    }
    let mut slots: Vec<Option<Candidate>> = cands.into_iter().map(Some).collect();
    groups
        .into_iter()
        .filter_map(|(score, rep)| {
            let mut h = slots[rep].take()?.hit;
            h.score = score;
            Some(h)
        })
        .collect()
}

/// Why the semantic half did not run. Always becomes a caveat, never an error.
struct SemanticSkipped(String);

#[cfg(test)]
mod scope_clause_tests {
    use super::bereich_clause;

    fn sql(bereich: Option<&str>, scope: Option<&[String]>) -> (String, usize) {
        let mut s = String::from("WHERE x MATCH ?1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new("q")];
        bereich_clause(&mut s, &mut args, bereich, scope);
        (s, args.len())
    }

    #[test]
    fn a_scope_is_an_in_list_after_the_bound_arguments_and_empty_admits_nothing() {
        let sc = vec!["seo".to_string(), "web".to_string()];
        assert_eq!(
            sql(Some("seo"), Some(&sc)),
            (
                "WHERE x MATCH ?1 AND n.bereich = ?2 AND n.bereich IN (?3,?4)".into(),
                4
            )
        );
        assert_eq!(sql(None, Some(&[])), ("WHERE x MATCH ?1 AND 0".into(), 1));
        assert_eq!(sql(None, None), ("WHERE x MATCH ?1".into(), 1));
    }
}
