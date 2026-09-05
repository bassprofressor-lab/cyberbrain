//! Mean pooling and L2 normalisation. Pure functions so they can be tested without a model.

/// Below this norm a pooled vector is treated as empty rather than scaled up into noise.
const MIN_NORM: f32 = 1e-12;

/// Sums the rows named by `ids` (already filtered to known ids) into `acc`, divides by the
/// count and L2-normalises in place. Returns the number of rows pooled.
///
/// If no row was pooled, or the mean is (numerically) the zero vector, `acc` is left all
/// zero and `0` is returned. The zero vector is the documented output for "nothing to
/// embed"; it is never NaN.
pub(crate) fn mean_pool_normalise<'a>(
    acc: &mut [f32],
    rows: impl Iterator<Item = &'a [f32]>,
) -> usize {
    debug_assert!(acc.iter().all(|x| *x == 0.0));
    let mut count = 0usize;
    for row in rows {
        for (a, w) in acc.iter_mut().zip(row) {
            *a += *w;
        }
        count += 1;
    }
    if count == 0 {
        return 0;
    }
    let inv = 1.0 / count as f32;
    for a in acc.iter_mut() {
        *a *= inv;
    }
    if !normalise(acc) {
        acc.fill(0.0);
        return 0;
    }
    count
}

/// Scales `v` to unit L2 norm in place. Returns `false`, leaving `v` untouched, when the
/// norm is too small or not finite to normalise meaningfully.
pub(crate) fn normalise(v: &mut [f32]) -> bool {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !(norm.is_finite() && norm > MIN_NORM) {
        return false;
    }
    let inv = 1.0 / norm;
    for x in v.iter_mut() {
        *x *= inv;
    }
    true
}

/// True when every component is exactly zero: the vector this crate returns for an empty
/// or all-unknown input. Callers should skip semantic search for such a query instead of
/// ranking on cosines that are all 0.
pub fn is_zero(v: &[f32]) -> bool {
    v.iter().all(|x| *x == 0.0)
}
