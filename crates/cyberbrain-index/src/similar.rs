//! Notes whose blocks lie close to a given note's blocks, for the write-time hint "does this
//! replace an older note?" (2026-11-24).
//!
//! Why it exists: recall ranks a replaced note below its successor only when somebody said
//! so (`supersedes` / `superseded_by`), and in the orderflow store 18 of 3,399 notes did. The
//! older note then keeps ranking first for the question its successor answers. The cheapest
//! moment to ask is the write itself: the new note's vectors were just computed and stored,
//! so no query is embedded and no model is called.
//!
//! This module only measures. Which neighbours are worth naming (same kind, older, similar
//! name) is the caller's judgement, because it needs the note's name and the client's scope.

use crate::vectors::{VectorCache, decode, normalize};
use crate::{Index, SqlResultExt};
use cyberbrain_core::{NoteId, Result, Ring};
use std::collections::HashMap;

/// A note near the one asked about.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbour {
    pub name: String,
    /// The kind as stored in the index (`knowledge`, `decision`, ...).
    pub kind: String,
    pub created: String,
    /// Already marked as replaced by someone else.
    pub superseded_by: Option<String>,
    /// The highest cosine between any block of the asked-about note and any block of this one.
    pub cosine: f32,
}

impl Index {
    /// Up to `k` other notes in `ring` whose best block pair with note `id` scores above
    /// `min_cosine`, best first. Empty when the note has no vectors or the index has no
    /// embedding profile: without a model there is nothing to compare, and that is not an
    /// error for a write.
    pub fn neighbours_of(
        &self,
        id: &NoteId,
        ring: Ring,
        k: usize,
        min_cosine: f32,
    ) -> Result<Vec<Neighbour>> {
        let Some(profile) = self.embedding_profile()? else {
            return Ok(Vec::new());
        };
        let own_id = id.to_string();
        let mut own: Vec<Vec<f32>> = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare_cached(
                    "SELECT v.data FROM vectors v JOIN blocks b ON b.citation = v.citation \
                     WHERE b.note_id = ?1",
                )
                .ix()?;
            let rows = stmt.query_map([&own_id], |r| r.get::<_, Vec<u8>>(0)).ix()?;
            for r in rows {
                let mut v = decode(&r.ix()?)?;
                if v.len() == profile.dim && normalize(&mut v) {
                    own.push(v);
                }
            }
        }
        if own.is_empty() || k == 0 {
            return Ok(Vec::new());
        }

        let generation = self.generation()?;
        let mut cache = self.cache.borrow_mut();
        if cache.as_ref().map(|c| c.generation) != Some(generation) {
            *cache = Some(VectorCache::load(&self.conn, generation, profile.dim)?);
        }
        let cache = cache.as_ref().expect("just loaded");

        // Best cosine per foreign block, then per note. A note's own blocks are among the
        // candidates and are dropped by note id below, so ask for a few more than `k`.
        let mut best_block: HashMap<usize, f32> = HashMap::new();
        for v in &own {
            for (s, i) in cache.top_k(v, k * 8 + own.len(), Some(ring), None, min_cosine) {
                let e = best_block.entry(i).or_insert(s);
                if s > *e {
                    *e = s;
                }
            }
        }
        let mut per_note: HashMap<String, Neighbour> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT n.id, n.name, n.kind, n.created, n.superseded_by FROM blocks b \
                 JOIN notes n ON n.id = b.note_id WHERE b.citation = ?1",
            )
            .ix()?;
        for (i, s) in best_block {
            let row = stmt
                .query_row([&cache.citations[i]], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })
                .ix();
            // A vector whose block is gone is `doctor`'s business, not a write's.
            let Ok((nid, name, kind, created, superseded_by)) = row else {
                continue;
            };
            if nid == own_id {
                continue;
            }
            let e = per_note.entry(nid).or_insert(Neighbour {
                name,
                kind,
                created,
                superseded_by,
                cosine: s,
            });
            if s > e.cosine {
                e.cosine = s;
            }
        }
        let mut out: Vec<Neighbour> = per_note.into_values().collect();
        out.sort_by(|a, b| {
            b.cosine
                .total_cmp(&a.cosine)
                .then_with(|| a.name.cmp(&b.name))
        });
        out.truncate(k);
        Ok(out)
    }
}
