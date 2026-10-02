//! Acceptance tests for `modhash-math`: FFT against a naive O(N²) DFT,
//! Parseval, roundtrip, DCT-II against the direct definition, the
//! pinned median convention, and 3×3 solve/inverse on known systems.
//!
//! Deterministic randomness comes from `modhash_primitives::SplitMix64`
//! — the workspace's one PRNG — never from the clock or `rand`.

use modhash_math::{
    Complex, IDENTITY3, dct2, dct2_2d, det3, fft, fft_real, idct2, idct2_2d, ifft, inverse3,
    mat3_mul, mat3_mul_vec, median, median_copy, solve3, transpose3,
};
use modhash_primitives::SplitMix64;
use std::f64::consts::PI;

/// Uniform-ish `f64` in `(-1, 1)` from the top 53 bits of `rng`.
fn rand_unit(rng: &mut SplitMix64) -> f64 {
    let bits = (rng.next_u64() >> 11) as f64; // 53-bit mantissa space
    (bits / (1u64 << 53) as f64) * 2.0 - 1.0
}
fn max_complex_err(a: &[Complex], b: &[Complex]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (*x - *y).norm())
        .fold(0.0, f64::max)
}

/// Largest element-wise `|a − b|` over two `f64` slices.
fn max_abs_err(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

/// The O(N²) DFT, `X[k] = Σ_j x[j]·e^(−2πi·j·k/N)`, computed straight
/// from the definition. This is the reference `fft` must match — the
/// two implementations share no code path, so agreement is real
/// evidence, not an echo.
fn naive_dft(x: &[Complex]) -> Vec<Complex> {
    let n = x.len();
    let mut out = vec![Complex::ZERO; n];
    for (k, slot) in out.iter_mut().enumerate() {
        let mut acc = Complex::ZERO;
        for (j, &xj) in x.iter().enumerate() {
            let w = Complex::cis(-2.0 * PI * (j * k) as f64 / n as f64);
            acc = acc + xj * w;
        }
        *slot = acc;
    }
    out
}

fn complex_signal(n: usize, seed: u64) -> Vec<Complex> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| Complex::new(rand_unit(&mut rng), rand_unit(&mut rng)))
        .collect()
}

fn real_signal(n: usize, seed: u64) -> Vec<f64> {
    let mut rng = SplitMix64::new(seed);
    (0..n).map(|_| rand_unit(&mut rng)).collect()
}

// ---------------------------------------------------------------------
// FFT
// ---------------------------------------------------------------------

#[test]
fn fft_matches_naive_dft_powers_of_two() {
    for shift in 0..=6 {
        let n = 1usize << shift; // 1..=64, every power of two
        let x = complex_signal(n, 0x5EED ^ n as u64);
        let mut buf = x.clone();
        fft(&mut buf);
        let want = naive_dft(&x);
        let err = max_complex_err(&buf, &want);
        // FFT error grows like O(ε·log₂N); 1e-9 is generous headroom
        // that a wrong-sign twiddle or dropped term blows through.
        assert!(err < 1e-9, "n={n}: fft vs naive DFT err {err:e}");
    }
    // The plan's acceptance sizes include 256.
    let x = complex_signal(256, 0x00C0_FFEE);
    let mut buf = x.clone();
    fft(&mut buf);
    let err = max_complex_err(&buf, &naive_dft(&x));
    assert!(err < 1e-9, "n=256: fft vs naive DFT err {err:e}");
}

#[test]
fn fft_parseval_holds() {
    for &n in &[4usize, 16, 64, 256, 1024] {
        let x = complex_signal(n, 0xBEEF + n as u64);
        let mut buf = x.clone();
        fft(&mut buf);
        let time: f64 = x.iter().map(|c| c.norm_sq()).sum();
        let freq: f64 = buf.iter().map(|c| c.norm_sq()).sum::<f64>() / n as f64;
        let rel = (time - freq).abs() / time.max(1e-30);
        assert!(rel < 1e-9, "n={n}: Parseval rel err {rel:e}");
    }
}

#[test]
fn fft_ifft_roundtrip() {
    for shift in 0..=8 {
        let n = 1usize << shift; // 1..=256
        let x = complex_signal(n, 0xABCD ^ ((n as u64) << 8));
        let mut buf = x.clone();
        fft(&mut buf);
        ifft(&mut buf);
        let err = max_complex_err(&buf, &x);
        assert!(err < 1e-9, "n={n}: ifft(fft(x)) err {err:e}");
    }
}

#[test]
fn fft_real_matches_complex_path_and_symmetry() {
    for &n in &[2usize, 8, 64, 256] {
        let x = real_signal(n, 0xF00D + n as u64);
        let bins = fft_real(&x);
        assert_eq!(bins.len(), n);
        // Same answer the complex path gives on the same signal.
        let mut buf: Vec<Complex> = x.iter().map(|&re| Complex::new(re, 0.0)).collect();
        fft(&mut buf);
        assert!(
            max_complex_err(&bins, &buf) < 1e-12,
            "n={n}: fft_real diverged"
        );
        // Conjugate symmetry: X[k] = conj(X[n−k]).
        for k in 1..(n / 2) {
            let mirror = bins[n - k].conj();
            let err = (bins[k] - mirror).norm();
            assert!(err < 1e-12, "n={n} k={k}: hermitian symmetry broken");
        }
        // Imaginary parts of the DC and Nyquist bins must vanish.
        assert!(bins[0].im.abs() < 1e-12);
        if n > 1 {
            assert!(bins[n / 2].im.abs() < 1e-12);
        }
    }
}

#[test]
fn fft_dc_and_dirac_have_exact_spectra() {
    // Constant signal: all energy in bin 0.
    let mut dc = vec![Complex::new(1.0, 0.0); 64];
    fft(&mut dc);
    assert!((dc[0] - Complex::new(64.0, 0.0)).norm() < 1e-12);
    for (k, bin) in dc.iter().enumerate().skip(1) {
        assert!(bin.norm() < 1e-9, "constant signal leaked into bin {k}");
    }
    // Dirac at 0: flat unit spectrum.
    let mut dirac = vec![Complex::ZERO; 64];
    dirac[0] = Complex::new(1.0, 0.0);
    fft(&mut dirac);
    for (k, bin) in dirac.iter().enumerate() {
        assert!(
            (*bin - Complex::new(1.0, 0.0)).norm() < 1e-12,
            "dirac bin {k} not 1"
        );
    }
}

// ---------------------------------------------------------------------
// DCT
// ---------------------------------------------------------------------

/// Independent DCT-II check via the mirrored DFT: for the length-N
/// signal x mirrored to 2N, F[k] = c_k·√(2/N)·½·Re(e^(−iπk/2N)·Y[k])
/// where Y is the plain DFT of the mirror. A genuinely different code
/// path from `dct2`'s direct cosine sum.
fn dct2_via_mirrored_dft(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mut mirror: Vec<Complex> = x.iter().map(|&v| Complex::new(v, 0.0)).collect();
    for i in (0..n).rev() {
        mirror.push(Complex::new(x[i], 0.0));
    }
    let y = naive_dft(&mirror);
    let scale = (2.0 / n as f64).sqrt();
    (0..n)
        .map(|k| {
            let phase = Complex::cis(-PI * k as f64 / (2.0 * n as f64));
            let c_k = if k == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            c_k * scale * 0.5 * (y[k] * phase).re
        })
        .collect()
}
#[test]
fn dct2_matches_mirrored_dft_reference() {
    // Odd sizes too: the kernel accepts any n, and the mirrored-DFT
    // identity holds for all of them.
    for &n in &[1usize, 2, 3, 5, 8, 13, 32, 64] {
        let x = real_signal(n, 0xDC70 ^ n as u64);
        let got = dct2(&x);
        let want = dct2_via_mirrored_dft(&x);
        let err = max_abs_err(&got, &want);
        assert!(err < 1e-9, "n={n}: dct2 vs mirrored-DFT err {err:e}");
    }
}

#[test]
fn dct2_in_place_matches_allocating_version() {
    let x = real_signal(8, 0x1D27_0008);
    let mut inplace = x.clone();
    let mut scratch = vec![0.0; inplace.len()];
    modhash_math::dct2_in_place(&mut inplace, &mut scratch);
    assert!(max_abs_err(&inplace, &dct2(&x)) < 1e-15);
}

#[test]
fn dct2_idct2_roundtrip() {
    for &n in &[1usize, 4, 8, 31, 64] {
        let x = real_signal(n, 0x1D27 + n as u64);
        let back = idct2(&dct2(&x));
        let err = max_abs_err(&back, &x);
        assert!(err < 1e-9, "n={n}: idct2(dct2(x)) err {err:e}");
    }
}

/// Direct O(w²·h²) evaluation of the separable 2D orthonormal DCT-II —
/// the literal definition, no shortcut, for a `w×h` row-major matrix.
fn naive_dct2_2d(m: &[f64], w: usize, h: usize) -> Vec<f64> {
    let mut out = vec![0.0; w * h];
    for u in 0..h {
        for v in 0..w {
            let mut acc = 0.0;
            for y in 0..h {
                for x in 0..w {
                    acc += m[y * w + x]
                        * (PI * (2 * x + 1) as f64 * v as f64 / (2.0 * w as f64)).cos()
                        * (PI * (2 * y + 1) as f64 * u as f64 / (2.0 * h as f64)).cos();
                }
            }
            let c_u = if u == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            let c_v = if v == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            out[u * w + v] = c_u * c_v * (2.0 / h as f64).sqrt() * (2.0 / w as f64).sqrt() * acc;
        }
    }
    out
}

#[test]
fn dct2_2d_matches_direct_definition() {
    // The spec's conformance shape: O(n⁴) reference on a seeded matrix.
    for &(w, h) in &[(8usize, 8usize), (4, 6), (1, 8), (8, 1), (3, 3)] {
        let m = real_signal(w * h, 0x2D00 ^ ((w as u64) << 8) ^ h as u64);
        let mut got = m.clone();
        dct2_2d(&mut got, w, h);
        let want = naive_dct2_2d(&m, w, h);
        let err = max_abs_err(&got, &want);
        assert!(err < 1e-9, "{w}x{h}: dct2_2d vs direct def err {err:e}");
    }
}

#[test]
fn dct2_2d_roundtrip_and_energy() {
    let mut m = real_signal(64, 0x8642_1357);
    // seeded 8×8 block — the pHash case
    let orig = m.clone();
    dct2_2d(&mut m, 8, 8);
    // Orthonormal ⇒ Parseval in the plane: ΣF² = Σm².
    let e_in: f64 = orig.iter().map(|v| v * v).sum();
    let e_out: f64 = m.iter().map(|v| v * v).sum();
    let rel = (e_in - e_out).abs() / e_in;
    assert!(rel < 1e-9, "2d energy not preserved: rel {rel:e}");
    idct2_2d(&mut m, 8, 8);
    assert!(max_abs_err(&m, &orig) < 1e-9, "2d roundtrip drifted");
}

#[test]
fn dct2_dc_coefficient_is_scaled_mean() {
    // Constant block c ⇒ F[0][0] = c·√(w·h) for the orthonormal form,
    // all other coefficients exactly zero.
    let mut m = vec![3.5_f64; 64];
    dct2_2d(&mut m, 8, 8);
    assert!((m[0] - 3.5 * 8.0).abs() < 1e-9, "DC wrong: {}", m[0]);
    for (i, &v) in m.iter().enumerate().skip(1) {
        assert!(v.abs() < 1e-12, "AC[{i}] leaked: {v:e}");
    }
}

#[test]
fn dct2_empty_and_scalar_inputs() {
    assert!(dct2(&[]).is_empty());
    // n=1: F[0] = x[0] (c_0·√2 rounds a hair off 1.0, so compare ≈).
    assert!((dct2(&[7.25])[0] - 7.25).abs() < 1e-12);
    assert!(idct2(&[]).is_empty());
    let mut m = vec![-2.0_f64];
    dct2_2d(&mut m, 1, 1);
    assert!((m[0] - -2.0).abs() < 1e-12);
    let mut empty: Vec<f64> = Vec::new();
    dct2_2d(&mut empty, 0, 0);
    idct2_2d(&mut empty, 0, 0);
    assert!(empty.is_empty());
}

// ---------------------------------------------------------------------
// Median — the pinned convention lives here
// ---------------------------------------------------------------------

#[test]
fn median_even_length_is_lower_middle() {
    // THE convention pin. Even-length median is sorted[n/2 − 1],
    // never the mean of the middle pair: [1,2,3,4] → 2, not 2.5.
    assert_eq!(median(&mut [1.0, 2.0, 3.0, 4.0]), Some(2.0));
    assert_eq!(median(&mut [10.0, 20.0]), Some(10.0));
    assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.0));
    // Ulp-close middles: an average could round to either member or
    // between them; the lower pick is exact and inside the data.
    let lo = 1.0;
    let hi = 1.0 + f64::EPSILON;
    assert_eq!(median(&mut [0.0, lo, hi, 2.0]), Some(1.0));
}

#[test]
fn median_odd_length_is_middle() {
    assert_eq!(median(&mut [1.0, 2.0, 3.0]), Some(2.0));
    assert_eq!(median(&mut [5.0]), Some(5.0));
    assert_eq!(median(&mut [9.0, -3.0, 4.0, 4.0, 7.0]), Some(4.0));
}

#[test]
fn median_empty_is_none() {
    assert_eq!(median(&mut []), None);
    assert_eq!(median_copy(&[]), None);
}

#[test]
fn median_is_an_order_statistic_not_an_interpolation() {
    // Patterned case the spec calls out: a median used as a threshold
    // must be a member of the set it thresholds. For [0,0,0,9,9,9]
    // the mean-of-middles is 4.5 — a member of nothing; the lower
    // middle is 0.
    assert_eq!(median(&mut [0.0, 0.0, 0.0, 9.0, 9.0, 9.0]), Some(0.0));
    // The 63-value pHash case: sorted[31] of sixty-three elements.
    let mut v: Vec<f64> = (0..63).map(|i| ((i * 37) % 63) as f64).collect();
    // i·37 mod 63 is a permutation of 0..63 (37 coprime to 63), so the
    // sorted vector is 0..=62 and the median is 31.
    assert_eq!(median(&mut v), Some(31.0));
}

#[test]
fn median_copy_leaves_input_untouched() {
    let src = [3.0, 1.0, 4.0, 1.0, 5.0];
    assert_eq!(median_copy(&src), Some(3.0));
    assert_eq!(src, [3.0, 1.0, 4.0, 1.0, 5.0]);
}

#[test]
fn median_sorts_deterministically_through_nan() {
    // total_cmp pins NaN at the top: [x, NaN, y, z] sorts so the NaN
    // never lands in the middle pair. Result is defined, not UB-ish.
    assert_eq!(median(&mut [1.0, f64::NAN, 2.0, 3.0]), Some(2.0));
    assert!(median(&mut [f64::NAN]).is_some_and(|m| m.is_nan()));
}

// ---------------------------------------------------------------------
// solve3 / linalg
// ---------------------------------------------------------------------

#[test]
fn solve3_known_systems() {
    // 2x + y − z = 8 · −3x − y + 2z = −11 · −2x + y + 2z = −3
    // has the textbook solution x = 2, y = 3, z = −1.
    let a = [[2.0, 1.0, -1.0], [-3.0, -1.0, 2.0], [-2.0, 1.0, 2.0]];
    let b = [8.0, -11.0, -3.0];
    let x = solve3(&a, &b).expect("well-conditioned system must solve");
    let want = [2.0, 3.0, -1.0];
    let err = max_abs_err(&x, &want);
    assert!(err < 1e-12, "solve3 drifted: {err:e}");

    // Identity round-trips the rhs.
    assert_eq!(
        solve3(&IDENTITY3, &[4.0, -2.0, 7.0]),
        Some([4.0, -2.0, 7.0])
    );

    // Permutation-needed system: a[0][0] = 0 forces a real pivot swap,
    // which is exactly what partial pivoting exists for.
    let a2 = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    assert_eq!(solve3(&a2, &[5.0, 6.0, 7.0]), Some([6.0, 5.0, 7.0]));
}

#[test]
fn solve3_residual_is_zero_on_random_systems() {
    let mut st = SplitMix64::new(0x5EED_5EED);
    for case in 0..64 {
        let mut a = [[0.0_f64; 3]; 3];
        for row in a.iter_mut() {
            for v in row.iter_mut() {
                *v = rand_unit(&mut st) * 10.0;
            }
        }
        // Nudge the diagonal so the random draw is well conditioned.
        a[0][0] += 4.0;
        a[1][1] += 4.0;
        a[2][2] += 4.0;
        let b = [rand_unit(&mut st), rand_unit(&mut st), rand_unit(&mut st)];
        let Some(x) = solve3(&a, &b) else {
            panic!("case {case}: diagonally-strong system reported singular");
        };
        // Residual ||a·x − b||∞ must be near machine epsilon × scale.
        let r = mat3_mul_vec(&a, &x);
        let err = max_abs_err(&r, &b);
        assert!(err < 1e-10, "case {case}: residual {err:e}");
    }
}

#[test]
fn solve3_singular_returns_none() {
    // Rank-2: third row is the sum of the first two.
    let a = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [5.0, 7.0, 9.0]];
    assert_eq!(solve3(&a, &[1.0, 1.0, 2.0]), None);
    // Duplicate rows.
    let dup = [[1.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 0.0]];
    assert_eq!(solve3(&dup, &[0.0, 0.0, 0.0]), None);
    // All-zero.
    assert_eq!(solve3(&[[0.0; 3]; 3], &[0.0; 3]), None);
    // A zero column is singular too.
    let zc = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    assert_eq!(solve3(&zc, &[1.0, 2.0, 3.0]), None);
}

#[test]
fn det3_known_values() {
    assert_eq!(det3(&IDENTITY3), 1.0);
    let m = [[6.0, 1.0, 1.0], [4.0, -2.0, 5.0], [2.0, 8.0, 7.0]];
    // Known determinant −306 (a standard worked example).
    assert_eq!(det3(&m), -306.0);
    // Singular ⇒ det 0.
    assert_eq!(
        det3(&[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [5.0, 7.0, 9.0]]),
        0.0
    );
    // det(A·B) = det(A)·det(B).
    let p = [[1.0, 2.0, 0.0], [0.0, 1.0, 3.0], [2.0, 0.0, 1.0]];
    let q = [[2.0, 0.0, 1.0], [1.0, 3.0, 0.0], [0.0, 1.0, 2.0]];
    let prod = det3(&mat3_mul(&p, &q));
    let expect = det3(&p) * det3(&q);
    assert!((prod - expect).abs() < 1e-12 * expect.abs().max(1.0));
}

#[test]
fn inverse3_inverts_and_mat3_mul_composes() {
    let a = [[2.0, 0.0, 1.0], [0.0, 3.0, 0.0], [1.0, 0.0, 2.0]];
    let inv = inverse3(&a).expect("invertible");
    let prod = mat3_mul(&a, &inv);
    for i in 0..3 {
        for j in 0..3 {
            let err = (prod[i][j] - IDENTITY3[i][j]).abs();
            assert!(err < 1e-12, "a·a⁻¹[{i}][{j}] = {}", prod[i][j]);
        }
    }
    // Transpose sanity.
    let t = transpose3(&a);
    assert_eq!(t, [[2.0, 0.0, 1.0], [0.0, 3.0, 0.0], [1.0, 0.0, 2.0]]);
    let b = [[1.0, 2.0, 3.0], [0.0, 1.0, 4.0], [5.0, 6.0, 0.0]];
    assert_eq!(transpose3(&transpose3(&b)), b);
    // Singular ⇒ None, same convention as solve3.
    assert_eq!(inverse3(&[[0.0; 3]; 3]), None);
    assert_eq!(
        inverse3(&[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [5.0, 7.0, 9.0]]),
        None
    );
}

#[test]
fn mat3_mul_vec_applies_the_matrix() {
    // Rotation by 90° about z: x→y, y→−x.
    let rot = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let v = mat3_mul_vec(&rot, &[1.0, 0.0, 0.0]);
    assert_eq!(v, [0.0, 1.0, 0.0]);
    // Composing two 90° rotations gives 180°.
    let rot2 = mat3_mul(&rot, &rot);
    let v2 = mat3_mul_vec(&rot2, &[1.0, 0.0, 0.0]);
    assert!(max_abs_err(&v2, &[-1.0, 0.0, 0.0]) < 1e-15);
}
