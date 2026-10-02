//! DCT-II and its inverse, 1D and separable 2D, over `f64`.
//!
//! The transform is the **orthonormal** DCT-II:
//!
//! ```text
//! F[k] = c_k · √(2/N) · Σ_n x[n] · cos(π·(2n+1)·k / (2N)),
//! c_0 = 1/√2, c_k = 1 otherwise
//! ```
//!
//! chosen because the orthonormal form makes the inverse a relabeled
//! DCT-III with matching scale — `dct2` followed by [`idct2`] is the
//! identity within a few ulps — and it is the normalization the kit's
//! pHash path documents.
//!
//! The implementation is the direct O(N²) double loop, **not** a fast
//! DCT built on [`crate::fft`]. That is deliberate: the consumers are
//! small fixed blocks (8×8 DCT cells in pHash and JPEG, 32×32 pHash
//! images) where a radix-style fast DCT would trade a few thousand
//! cycles for a second code path that must itself be proven equivalent
//! to the definition. The direct sum of cosines *is* the definition the
//! conformance vectors check. If a profile ever shows DCT time
//! mattering, this file is the place to change — the interface and the
//! test vectors stay the same.
//!
//! The 2D form [`dct2_2d`] is separable: 1D kernel on every row, then on
//! every column — the "rows then columns" order the pHash pipeline
//! names. For the orthonormal kernel the inverse has the same shape
//! with the transposed kernel matrix (DCT-III), so [`idct2_2d`] reuses
//! the identical driver: `Aᵀ·F·A` where the forward was `A·M·Aᵀ`.

use std::f64::consts::PI;

/// 1D orthonormal DCT-II of `x`, returned as a fresh `Vec`.
///
/// `F[k] = c_k·√(2/N)·Σ_n x[n]·cos(π·(2n+1)·k/(2N))`.
/// An empty input returns an empty vector.
#[must_use]
pub fn dct2(x: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; x.len()];
    dct2_into(x, &mut out);
    out
}

/// In-place 1D DCT-II: replaces `data` with its transform using
/// `scratch` (same length) as workspace, so hot loops over fixed-size
/// blocks never allocate.
///
/// # Panics
///
/// Panics if `scratch.len() != data.len()`.
pub fn dct2_in_place(data: &mut [f64], scratch: &mut [f64]) {
    assert_eq!(
        scratch.len(),
        data.len(),
        "dct2 scratch must match data length"
    );
    dct2_into(data, scratch);
    data.copy_from_slice(scratch);
}

/// 1D orthonormal DCT-III — the exact inverse of [`dct2`].
///
/// `x[n] = √(2/N)·(F[0]/√2 + Σ_{k≥1} F[k]·cos(π·(2n+1)·k/(2N)))`.
/// The same O(N²) reasoning as [`dct2`] applies: this is the direct
/// sum, because the blocks are small and the direct sum *is* the
/// definition.
#[must_use]
pub fn idct2(f: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; f.len()];
    idct2_into(f, &mut out);
    out
}

/// In-place separable 2D DCT-II on a `w × h` row-major matrix.
///
/// Applies the 1D kernel to every row, then to every column — the
/// "rows then columns" order the pHash spec names, i.e.
/// `F = A·M·Aᵀ` with `A` the 1D kernel matrix.
///
/// One scratch buffer of `2·max(w, h)` elements is allocated per call;
/// block sizes in the kit (8, 32) make that negligible.
///
/// # Panics
///
/// Panics if `data.len() != w * h`, or on a zero dimension with
/// non-empty `data`.
pub fn dct2_2d(data: &mut [f64], w: usize, h: usize) {
    separable(data, w, h, dct2_into);
}

/// In-place separable 2D inverse (DCT-III on both axes), the exact
/// inverse of [`dct2_2d`].
///
/// Because the kernel matrix `A` is orthonormal, `F = A·M·Aᵀ` inverts
/// to `M = Aᵀ·F·A` — the same rows-then-columns driver with the DCT-III
/// kernel `Aᵀ` in place of `A`. No transposes, no second code shape.
///
/// # Panics
///
/// Same shape contract as [`dct2_2d`].
pub fn idct2_2d(data: &mut [f64], w: usize, h: usize) {
    separable(data, w, h, idct2_into);
}

/// Shared rows-then-columns driver: run `kernel` on each `w`-element
/// row, then on each `h`-element column, gathering through one scratch.
fn separable(data: &mut [f64], w: usize, h: usize, kernel: fn(&[f64], &mut [f64])) {
    assert_eq!(data.len(), w * h, "dct2 2d shape mismatch");
    if data.is_empty() {
        return;
    }
    assert!(w > 0 && h > 0, "dct2 2d zero dimension");
    let m = w.max(h);
    // scratch[0..m] gathers a column; scratch[m..2m] is the kernel's
    // own output buffer (the kernel may not alias its input).
    let mut scratch = vec![0.0; 2 * m];
    let (gather, out) = scratch.split_at_mut(m);

    for row in data.chunks_exact_mut(w) {
        kernel(row, &mut out[..w]);
        row.copy_from_slice(&out[..w]);
    }
    for x in 0..w {
        for y in 0..h {
            gather[y] = data[y * w + x];
        }
        kernel(&gather[..h], &mut out[..h]);
        for y in 0..h {
            data[y * w + x] = out[y];
        }
    }
}

/// The orthonormal DCT-II kernel: `out = DCT-II(x)`, `out` disjoint
/// from `x`. O(N²) — see the module note for why that is intended.
fn dct2_into(x: &[f64], out: &mut [f64]) {
    let n = x.len();
    debug_assert_eq!(out.len(), n);
    if n == 0 {
        return;
    }
    let scale = (2.0 / n as f64).sqrt();
    for (k, slot) in out.iter_mut().enumerate() {
        let mut acc = 0.0;
        for (i, &xi) in x.iter().enumerate() {
            acc += xi * (PI * ((2 * i + 1) * k) as f64 / (2.0 * n as f64)).cos();
        }
        let c_k = if k == 0 {
            std::f64::consts::FRAC_1_SQRT_2
        } else {
            1.0
        };
        *slot = c_k * scale * acc;
    }
}

/// The orthonormal DCT-III kernel: `out = DCT-III(f)`, the transpose
/// (hence inverse) of [`dct2_into`].
fn idct2_into(f: &[f64], out: &mut [f64]) {
    let n = f.len();
    debug_assert_eq!(out.len(), n);
    if n == 0 {
        return;
    }
    let scale = (2.0 / n as f64).sqrt();
    for (i, slot) in out.iter_mut().enumerate() {
        // k = 0 carries the halved weight of the orthonormal form.
        let mut acc = f[0] * std::f64::consts::FRAC_1_SQRT_2;
        for (k, &fk) in f.iter().enumerate().skip(1) {
            acc += fk * (PI * ((2 * i + 1) * k) as f64 / (2.0 * n as f64)).cos();
        }
        *slot = scale * acc;
    }
}
