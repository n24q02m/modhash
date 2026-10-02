//! Order statistics over `f64`: the median the kit's thresholds use.
//!
//! The pinned convention — and the line in this crate most often gotten
//! wrong — is the **even-length rule**: for `n` even the median is the
//! **lower** of the two middle elements, `sorted[n / 2 - 1]`, never the
//! average of the two middle elements. `[1, 2, 3, 4]` medians to `2`,
//! not `2.5`.
//!
//! Two reasons drive the choice, and both are documented so a future
//! "obviously the mean is better" edit has to argue with them:
//!
//! * The median is only ever used as a *threshold against members of
//!   the same set* (pHash compares `dct > median` over the same 63
//!   coefficients, RANSAC compares residuals against their median).
//!   Averaging the two middles invents a value that is a member of no
//!   input and splits the middle pair arbitrarily; taking an actual
//!   order statistic keeps the threshold inside the data.
//! * An average can round *through* the gap: for middles `a` and `b`
//!   that differ by an ulp-scale amount, `(a + b) / 2` can equal `a`,
//!   `b`, or a value between them depending on rounding, which makes
//!   the threshold sensitive to addition rounding instead of to the
//!   data alone. The lower-middle pick is exact.
//!
//! Sorting uses [`f64::total_cmp`], the total order: `-0.0` sorts below
//! `+0.0` and `NaN` sorts after everything, so the result is defined for
//! every input — deterministic, never a panic on a NaN the way
//! `partial_cmp().unwrap()` would be.

/// Median of `v`, or `None` for an empty slice. **Sorts `v` in place.**
///
/// For odd `n` this is `sorted[n / 2]`; for even `n` it is the lower
/// middle element `sorted[n / 2 - 1]` — see the module note for why the
/// two middles are never averaged.
///
/// Callers that must keep their buffer use [`median_copy`]. Sorting in
/// place is intentional: the kit's callers hold scratch blocks they can
/// destroy, and in-place avoids an allocation per threshold.
#[must_use]
pub fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    // (n - 1) / 2 == n / 2 for odd n, n / 2 - 1 for even n: one index
    // expression for both cases, always the lower middle of the pair.
    Some(v[(n - 1) / 2])
}
/// Same even-length lower-middle convention as [`median`].
#[must_use]
pub fn median_copy(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut owned = v.to_vec();
    median(&mut owned)
}
