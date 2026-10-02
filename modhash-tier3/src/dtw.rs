//! Dynamic time warping with a Sakoe–Chiba band — spec §4.3: *"chuỗi
//! đặc trưng 1D, dải Sakoe–Chiba `w = 10%`, O(n·w) — không O(n²)"*.
//!
//! [`dtw`] consumes an explicit `n×m` cost matrix (row-major) so the
//! crate stays metric-agnostic; [`dtw_series`] is the spec's 1-D
//! feature-string convenience wrapper using absolute distance.
//!
//! Two deviations from textbook DTW, both pinned by the reference
//! implementation (`hashkit/tier3.py:dtw_mean_cost`) because they were
//! measured to matter:
//!
//! * The answer is the **per-step mean** `D[n][m] / L[n][m]`, not the
//!   raw total — normalizing by the warping path's length stops longer
//!   paths from looking worse just for being longer.
//! * The band **centres on the diagonal mapped by length ratio**,
//!   `centre_i = i·m/n`, so unequal-length sequences still get a usable
//!   corridor. `width = max(2, band·max(n, m))` gives the spec's 10 %
//!   band a 2-cell floor that keeps tiny inputs reachable.
//! * A pair whose band makes `(n, m)` unreachable answers `1.0` — the
//!   maximum-meaningful cost — rather than an error: "this band rejects
//!   the alignment" is a legitimate DTW verdict, not malformed input.
//!
//! The DP needs only the previous row, so it keeps two `m+1` buffers and
//! runs in `O(n·w)` time and `O(m)` space.

use crate::Error;

/// Spec §4.3 band: 10 % of the longer side.
pub const SAKOE_CHIBA_BAND: f64 = 0.10;

/// Mean cost per step of the cheapest warping path through `cost`, an
/// `n×m` row-major matrix, under a Sakoe–Chiba band of fraction `band`
/// (see [`SAKOE_CHIBA_BAND`]). `band < 0` disables the band entirely —
/// full `O(n·m)` DTW, kept for reference comparisons in tests.
///
/// Returns `Err(Error::BadValue)` when `cost.len() != n * m` — a
/// malformed matrix is a caller bug, not data. Empty sequences and
/// band-unreachable endpoints return `Ok(1.0)`.
pub fn dtw(cost: &[f64], n: usize, m: usize, band: f64) -> Result<f64, Error> {
    if cost.len() != n.saturating_mul(m) {
        return Err(Error::BadValue("cost matrix length != n*m"));
    }
    if n == 0 || m == 0 {
        return Ok(1.0);
    }
    let unbanded = band < 0.0;
    let width = (band * n.max(m) as f64).max(2.0);

    // Row 0 of the DP is the virtual origin: D[0][0] = 0 and L[0][0] is
    // a valid length-0 anchor; the rest of both buffers is "unreachable".
    let mut prev_d = vec![f64::INFINITY; m + 1];
    let mut prev_l = vec![0i64; m + 1];
    let mut cur_d = vec![f64::INFINITY; m + 1];
    let mut cur_l = vec![0i64; m + 1];
    prev_d[0] = 0.0;

    for i in 1..=n {
        // Banded window for this row: centre on the length-mapped
        // diagonal, clamped to [1, m]. The window is what makes the
        // band binding — everything outside stays INFINITY.
        let (lo, hi) = if unbanded {
            (1usize, m)
        } else {
            let centre = i as f64 * m as f64 / n as f64;
            (
                (centre - width).max(1.0) as usize,
                (centre + width).min(m as f64) as usize,
            )
        };
        cur_d.fill(f64::INFINITY);
        cur_l.fill(0);
        for j in lo..=hi {
            // Cheapest predecessor of (i, j): diagonal, up, left.
            let mut d = prev_d[j - 1];
            let mut steps = prev_l[j - 1];
            if prev_d[j] < d {
                d = prev_d[j];
                steps = prev_l[j];
            }
            if cur_d[j - 1] < d {
                d = cur_d[j - 1];
                steps = cur_l[j - 1];
            }
            if !d.is_finite() {
                continue;
            }
            cur_d[j] = d + cost[(i - 1) * m + (j - 1)];
            cur_l[j] = steps + 1;
        }
        core::mem::swap(&mut prev_d, &mut cur_d);
        core::mem::swap(&mut prev_l, &mut cur_l);
    }

    if !prev_d[m].is_finite() || prev_l[m] == 0 {
        return Ok(1.0);
    }
    Ok(prev_d[m] / prev_l[m] as f64)
}

/// [`dtw`] over the absolute-distance matrix of two 1-D sequences —
/// the spec's "chuỗi đặc trưng 1D" entry point.
pub fn dtw_series(a: &[f64], b: &[f64], band: f64) -> Result<f64, Error> {
    let (n, m) = (a.len(), b.len());
    let mut cost = Vec::with_capacity(n.saturating_mul(m));
    for &x in a {
        for &y in b {
            cost.push((x - y).abs());
        }
    }
    dtw(&cost, n, m, band)
}
