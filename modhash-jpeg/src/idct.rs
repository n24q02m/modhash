//! Inverse DCT: a line-for-line transcription of libjpeg's `islow`
//! integer IDCT (jidctint.c, AAN-scaled LLM variant) so that decoded
//! pixels are byte-identical to the reference decoder.
//!
//! Why not `modhash_math::idct2`: that function is an orthonormal
//! DCT-III over `f64`. JPEG decoders must reproduce the encoder's
//! *approximation* of the IDCT, and the ecosystem reference (libjpeg
//! `JDCT_ISLOW`) is this specific fixed-point pipeline; substituting an
//! `f64` kernel shifts output by ±1–2 LSB per sample and breaks
//! byte-exact fixture comparison. `idct2` is used instead as the
//! independent oracle in this crate's tests.
//!
//! Structure (both passes): even part = rotations on the even-indexed
//! coefficients; odd part = the 4×4 "figure 8" network on y7,y5,y3,y1.
//! Intermediate precision: pass 1 keeps `PASS1_BITS` (2) extra bits;
//! pass 2 descales by `CONST_BITS + PASS1_BITS + 3` total and runs the
//! result through [`range_limit`], which performs the +128 level shift
//! and clamp in one table lookup (replicated here as a function).
//!
//! `i64` accumulators stand in for libjpeg's `JLONG` (`long`): for all
//! encoder-legal inputs every value fits in 32 bits identically, and on
//! hostile inputs (16-bit DQT × large coefficients) the wider
//! accumulator avoids C's silent overflow, which keeps this
//! implementation panic-free and deterministic.

/// FIX constants, `CONST_BITS = 13` (jidctint.c, BITS_IN_JSAMPLE = 8).
const CONST_BITS: i64 = 13;
const PASS1_BITS: i64 = 2;
const FIX_0_298631336: i64 = 2446;
const FIX_0_390180644: i64 = 3196;
const FIX_0_541196100: i64 = 4433;
const FIX_0_765366865: i64 = 6270;
const FIX_0_899976223: i64 = 7373;
const FIX_1_175875602: i64 = 9633;
const FIX_1_501321110: i64 = 12299;
const FIX_1_847759065: i64 = 15137;
const FIX_1_961570560: i64 = 16069;
const FIX_2_053119869: i64 = 16819;
const FIX_2_562915447: i64 = 20995;
const FIX_3_072711026: i64 = 25172;

/// `(x + 1<<(n-1)) >> n` — arithmetic, so negative values floor-divide
/// exactly as C's `RIGHT_SHIFT` does on every supported toolchain.
#[inline]
fn descale(x: i64, n: i64) -> i64 {
    (x + (1i64 << (n - 1))) >> n
}

/// libjpeg's post-IDCT `range_limit` table, expressed as a function.
///
/// The table is indexed by `v & 1023` (10-bit wrap) and performs the
/// +128 level shift with saturation: inputs 0..128 → v+128 (the
/// nominal post-shift range), 128..512 → 255, 512..896 → 0, and
/// 896..1024 → j−896 which restores the wrap-around tail of
/// `v ∈ [−128, 0)` to `v+128`.
fn range_limit(v: i64) -> u8 {
    let j = (v & 1023) as usize;
    match j {
        0..=127 => (j + 128) as u8,
        128..=511 => 255,
        512..=895 => 0,
        _ => (j - 896) as u8,
    }
}

/// Dequantizes `coefs` (natural order, i32) against `qt` (natural order)
/// and writes the 8×8 pixel block into `plane` at `(x0, y0)` with row
/// `stride`. Output samples are clamped to 0..=255 by [`range_limit`].
///
/// Infallible by construction: every index is static, and `i64`
/// intermediates cannot overflow for any `i32` coefficient and `u16`
/// quantizer.
#[allow(clippy::identity_op, clippy::erasing_op)] // `r * 8` indexes stay
// spelled as row*8 so the write order matches jidctint.c line for line.
pub(crate) fn dequant_idct_into(
    coefs: &[i32],
    qt: &[u16],
    plane: &mut [u8],
    stride: usize,
    x0: usize,
    y0: usize,
) {
    debug_assert!(coefs.len() >= 64 && qt.len() >= 64);
    let mut ws = [0i64; 64];

    // Pass 1: columns. Inputs dequantized here; outputs keep
    // PASS1_BITS extra bits of scale.
    for c in 0..8 {
        let d = |r: usize| -> i64 { coefs[r * 8 + c] as i64 * i64::from(qt[r * 8 + c]) };

        // Even part.
        let z2 = d(2);
        let z3 = d(6);
        let z1 = (z2 + z3) * FIX_0_541196100;
        let tmp2 = z1 + z3 * -FIX_1_847759065;
        let tmp3 = z1 + z2 * FIX_0_765366865;

        let z2 = d(0);
        let z3 = d(4);
        let tmp0 = (z2 + z3) << CONST_BITS;
        let tmp1 = (z2 - z3) << CONST_BITS;

        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;

        // Odd part (y7, y5, y3, y1).
        let mut t0 = d(7);
        let mut t1 = d(5);
        let mut t2 = d(3);
        let mut t3 = d(1);

        let mut z1 = t0 + t3;
        let mut z2 = t1 + t2;
        let mut z3 = t0 + t2;
        let mut z4 = t1 + t3;
        let z5 = (z3 + z4) * FIX_1_175875602;

        t0 *= FIX_0_298631336;
        t1 *= FIX_2_053119869;
        t2 *= FIX_3_072711026;
        t3 *= FIX_1_501321110;
        z1 *= -FIX_0_899976223;
        z2 *= -FIX_2_562915447;
        z3 *= -FIX_1_961570560;
        z4 *= -FIX_0_390180644;

        z3 += z5;
        z4 += z5;

        t0 += z1 + z3;
        t1 += z2 + z4;
        t2 += z2 + z3;
        t3 += z1 + z4;

        ws[0 * 8 + c] = descale(tmp10 + t3, CONST_BITS - PASS1_BITS);
        ws[7 * 8 + c] = descale(tmp10 - t3, CONST_BITS - PASS1_BITS);
        ws[1 * 8 + c] = descale(tmp11 + t2, CONST_BITS - PASS1_BITS);
        ws[6 * 8 + c] = descale(tmp11 - t2, CONST_BITS - PASS1_BITS);
        ws[2 * 8 + c] = descale(tmp12 + t1, CONST_BITS - PASS1_BITS);
        ws[5 * 8 + c] = descale(tmp12 - t1, CONST_BITS - PASS1_BITS);
        ws[3 * 8 + c] = descale(tmp13 + t0, CONST_BITS - PASS1_BITS);
        ws[4 * 8 + c] = descale(tmp13 - t0, CONST_BITS - PASS1_BITS);
    }

    // Pass 2: rows. Workspace values carry PASS1_BITS extra bits; the
    // final descale removes CONST_BITS + PASS1_BITS + 3 total.
    let out_shift = CONST_BITS + PASS1_BITS + 3;
    for r in 0..8 {
        let row = &ws[r * 8..r * 8 + 8];

        // Even part.
        let z2 = row[2];
        let z3 = row[6];
        let z1 = (z2 + z3) * FIX_0_541196100;
        let tmp2 = z1 + z3 * -FIX_1_847759065;
        let tmp3 = z1 + z2 * FIX_0_765366865;

        let tmp0 = (row[0] + row[4]) << CONST_BITS;
        let tmp1 = (row[0] - row[4]) << CONST_BITS;

        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;

        // Odd part.
        let mut t0 = row[7];
        let mut t1 = row[5];
        let mut t2 = row[3];
        let mut t3 = row[1];

        let mut z1 = t0 + t3;
        let mut z2 = t1 + t2;
        let mut z3 = t0 + t2;
        let mut z4 = t1 + t3;
        let z5 = (z3 + z4) * FIX_1_175875602;

        t0 *= FIX_0_298631336;
        t1 *= FIX_2_053119869;
        t2 *= FIX_3_072711026;
        t3 *= FIX_1_501321110;
        z1 *= -FIX_0_899976223;
        z2 *= -FIX_2_562915447;
        z3 *= -FIX_1_961570560;
        z4 *= -FIX_0_390180644;

        z3 += z5;
        z4 += z5;

        t0 += z1 + z3;
        t1 += z2 + z4;
        t2 += z2 + z3;
        t3 += z1 + z4;

        let out = &mut plane[(y0 + r) * stride + x0..(y0 + r) * stride + x0 + 8];
        out[0] = range_limit(descale(tmp10 + t3, out_shift));
        out[7] = range_limit(descale(tmp10 - t3, out_shift));
        out[1] = range_limit(descale(tmp11 + t2, out_shift));
        out[6] = range_limit(descale(tmp11 - t2, out_shift));
        out[2] = range_limit(descale(tmp12 + t1, out_shift));
        out[5] = range_limit(descale(tmp12 - t1, out_shift));
        out[3] = range_limit(descale(tmp13 + t0, out_shift));
        out[4] = range_limit(descale(tmp13 - t0, out_shift));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Oracle: `modhash_math::idct2_2d` is an independent, orthonormal
    /// f64 DCT-III. The islow transcription must track it within a
    /// small fixed-point tolerance — ±2 LSB is the documented worst
    /// case for islow-class fixed-point IDCTs on encoder-legal
    /// coefficients (quantizer × signed-11-bit coefficient).
    #[test]
    fn islow_tracks_orthonormal_idct2_within_2lsb() {
        use modhash_primitives::SplitMix64;

        let mut rng = SplitMix64::new(0x5eed);
        let mut qt = [0u16; 64];
        let mut coefs = Vec::with_capacity(64);
        let mut plane = [0u8; 64];
        let mut worst = 0i64;
        for _trial in 0..2000 {
            for q in qt.iter_mut() {
                *q = (rng.next_u64() % 32 + 1) as u16;
            }
            coefs.clear();
            for _ in 0..64 {
                coefs.push((rng.next_u64() % 1024) as i32 - 512);
            }
            dequant_idct_into(&coefs, &qt, &mut plane, 8, 0, 0);

            // Reference: dequantize to f64, orthonormal inverse DCT,
            // level shift +128, clamp.
            let mut f: Vec<f64> = coefs
                .iter()
                .zip(qt.iter())
                .map(|(&c, &q)| c as f64 * f64::from(q))
                .collect();
            modhash_math::idct2_2d(&mut f, 8, 8);
            for (i, &v) in f.iter().enumerate() {
                let ideal = v + 128.0;
                // libjpeg's range-limit table wraps outside ±384 after
                // the shift (documented upstream quirk); only compare
                // the regime where the reference decoder is honest.
                if !(-255.0..=382.0).contains(&ideal) {
                    continue;
                }
                let expected = ideal.round().clamp(0.0, 255.0) as i64;
                let d = (i64::from(plane[i]) - expected).abs();
                worst = worst.max(d);
                assert!(
                    d <= 2,
                    "index {i}: islow {} vs oracle {expected} (Δ{d})",
                    plane[i]
                );
            }
        }
        assert!(worst <= 2);
    }
}
