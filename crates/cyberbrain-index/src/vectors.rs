//! Vector storage and the flat cosine scan (SPEC §7).
//!
//! Vectors are stored as little-endian `f32` blobs, L2-normalised on the way in, so cosine
//! similarity is a plain dot product. Search is a linear scan over one contiguous `Vec<f32>`
//! held in memory: at tens of thousands of blocks that is single-digit milliseconds, it
//! cannot go stale, and it is the correctness reference for any ANN index added later.

use crate::SqlResultExt;
use cyberbrain_core::{Citation, Error, Result, Ring};
use rusqlite::Connection;

/// Encode as little-endian bytes, independent of host endianness.
pub fn encode(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode little-endian bytes. Rejects a blob whose length is not a multiple of four.
pub fn decode(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::Index(format!(
            "vector blob has {} bytes, not a multiple of 4",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    decode_into(bytes, &mut out);
    Ok(out)
}

/// Append the decoded floats to `out`. Length must already be a multiple of four.
fn decode_into(bytes: &[u8], out: &mut Vec<f32>) {
    let (chunks, _) = bytes.as_chunks::<4>();
    out.extend(chunks.iter().map(|c| f32::from_le_bytes(*c)));
}

/// L2-normalise in place. Returns `false` (leaving the vector untouched) when the norm is
/// zero or not finite, so a degenerate vector scores 0 against everything instead of NaN.
pub fn normalize(v: &mut [f32]) -> bool {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !(norm.is_finite() && norm > 0.0) {
        return false;
    }
    // Already unit length: leave the bytes alone so a stored vector round-trips exactly.
    if (norm - 1.0).abs() < 1e-6 {
        return true;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    true
}

/// Dot product with eight independent accumulators. The shape autovectorises on every
/// target we ship for; the summation order is fixed, so results are bit-reproducible.
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut acc = [0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    let mut s = 0f32;
    for v in acc {
        s += v;
    }
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

/// All stored vectors, contiguous, with their citation and ring in parallel arrays.
/// Loaded lazily by `Index::recall` and reloaded whenever `meta.generation` changes, so a
/// write from another process is picked up on the next query.
#[derive(Debug)]
pub(crate) struct VectorCache {
    pub generation: i64,
    pub dim: usize,
    pub data: Vec<f32>,
    pub citations: Vec<String>,
    pub rings: Vec<Ring>,
}

impl VectorCache {
    pub fn load(conn: &Connection, generation: i64, dim: usize) -> Result<Self> {
        // No join: the ring is the first component of the citation, and a vector whose
        // block or note is gone must surface as a loud error at materialisation, not be
        // quietly skipped here. `integrity()` reports such orphans for `doctor`.
        let mut stmt = conn
            .prepare_cached("SELECT citation, dim, data FROM vectors")
            .ix()?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })
            .ix()?;
        let expected: usize = conn
            .query_row("SELECT count(*) FROM vectors", [], |r| r.get::<_, i64>(0))
            .ix()? as usize;
        let mut cache = VectorCache {
            generation,
            dim,
            data: Vec::with_capacity(expected * dim),
            citations: Vec::with_capacity(expected),
            rings: Vec::with_capacity(expected),
        };
        for row in rows {
            let (citation, row_dim, blob) = row.ix()?;
            if row_dim as usize != dim || blob.len() != dim * 4 {
                return Err(Error::Index(format!(
                    "vector for {citation} has dim {row_dim} and {} bytes, the embedding \
                     profile says dim {dim}; run `cyberbrain scan --full`",
                    blob.len()
                )));
            }
            decode_into(&blob, &mut cache.data);
            cache.rings.push(citation.parse::<Citation>()?.ring);
            cache.citations.push(citation);
        }
        Ok(cache)
    }

    pub fn len(&self) -> usize {
        self.citations.len()
    }

    /// Top `k` by cosine, highest first; ties broken by citation so the order is stable
    /// across rebuilds. `ring` restricts to one ring; `min_cosine` is a strict floor.
    pub fn top_k(
        &self,
        query: &[f32],
        k: usize,
        ring: Option<Ring>,
        min_cosine: f32,
    ) -> Vec<(f32, usize)> {
        if k == 0 || self.len() == 0 || query.len() != self.dim {
            return Vec::new();
        }
        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(self.len());
        for (i, row) in self.data.chunks_exact(self.dim).enumerate() {
            if let Some(r) = ring
                && self.rings[i] != r
            {
                continue;
            }
            let s = dot(query, row);
            if s > min_cosine {
                scored.push((s, i));
            }
        }
        let by_rank = |a: &(f32, usize), b: &(f32, usize)| {
            b.0.total_cmp(&a.0)
                .then_with(|| self.citations[a.1].cmp(&self.citations[b.1]))
        };
        if scored.len() > k {
            scored.select_nth_unstable_by(k, by_rank);
            scored.truncate(k);
        }
        scored.sort_by(by_rank);
        scored
    }
}
