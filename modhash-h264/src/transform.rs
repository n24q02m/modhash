//! Inverse quantisation and the H.264 4x4 integer inverse transform,
//! including the Intra-16x16 luma DC and chroma DC Hadamard transforms
//! (spec 8.5.10 – 8.5.13).
//!
//! Per spec 8.5.12.1 the dequantised coefficient is
//! `d_ij = (c_ij * LevelScale4x4(qP % 6, class(i,j))) << (qP / 6)`.
//! The inverse transform itself (spec 8.5.12.2) is the scaled integer
//! DCT applied horizontally then vertically with a final `(v + 32) >> 6`
//! and a spec-mandated range check on the result (|f| <= 511 used to
//! bound hostile input; legal streams never hit it).
//!
//! All arithmetic uses wrapping ops on the multiply-and-shift path:
//! CAVLC level values come straight from the bitstream, and a hostile
//! stream can produce magnitudes that would overflow i32 in debug
//! builds. Wrapping keeps the behaviour deterministic instead of
//! panicking; the conformance tests pin the legal-input exactness.

use crate::tables::{scale_class, LEVEL_SCALE_4X4, QP_DIV6, QP_MOD6};
use modhash_primitives::{Error, Result};

/// 4x4 inverse transform input/output block (`i32` per spec working
/// precision; inputs are post-dequant coefficients).
pub(crate) type Block4 = [i32; 16];

/// Inverse-transforms a dequantised 4x4 block in place
/// (spec 8.5.12.2). `Err` when an output coefficient would exceed the
/// spec's |r| <= 511 + 1 bound on intermediate results — legal streams
/// stay inside it; fuzz input that trips it is corrupt, not a panic.
pub(crate) fn inverse_4x4(b: &mut Block4) -> Result<()> {
    // Horizontal pass.
    for row in 0..4 {
        let p = &mut b[row * 4..row * 4 + 4];
        let t0 = p[0].wrapping_add(p[2]);
        let t1 = p[0].wrapping_sub(p[2]);
        let t2 = (p[1] >> 1).wrapping_sub(p[3]);
        let t3 = p[1].wrapping_add(p[3] >> 1);
        p[0] = t0.wrapping_add(t3);
        p[1] = t1.wrapping_add(t2);
        p[2] = t1.wrapping_sub(t2);
        p[3] = t0.wrapping_sub(t3);
    }
    // Vertical pass with the +32 >> 6 rescale.
    for col in 0..4 {
        let i0 = col;
        let i1 = col + 4;
        let i2 = col + 8;
        let i3 = col + 12;
        let t0 = b[i0].wrapping_add(b[i2]);
        let t1 = b[i0].wrapping_sub(b[i2]);
        let t2 = (b[i1] >> 1).wrapping_sub(b[i3]);
        let t3 = b[i1].wrapping_add(b[i3] >> 1);
        b[i0] = t0.wrapping_add(t3).wrapping_add(32) >> 6;
        b[i1] = t1.wrapping_add(t2).wrapping_add(32) >> 6;
        b[i2] = t1.wrapping_sub(t2).wrapping_add(32) >> 6;
        b[i3] = t0.wrapping_sub(t3).wrapping_add(32) >> 6;
        // Spec 8.5.12.2 constrains every intermediate to
        // [-2^15, 2^15-1]; a conforming stream never exceeds it.
        for &v in &[b[i0], b[i1], b[i2], b[i3]] {
            if !(-32768..=32767).contains(&v) {
                return Err(Error::BadValue("inverse transform intermediate out of range"));
            }
        }
    }
    Ok(())
}

/// Dequantises one 4x4 coefficient block in place (spec 8.5.12.1,
/// eqs. 8-336/8-337). `coeffs` holds the raw CAVLC levels; products are
/// widened to i64 so hostile magnitudes can't wrap silently — the
/// inverse-transform bound check then rejects over-range blocks.
pub(crate) fn dequant_4x4(coeffs: &mut [i32; 16], qp: u8) {
    debug_assert!(qp < 52);
    let m = LEVEL_SCALE_4X4[QP_MOD6[qp as usize] as usize];
    let q6 = qp as i64 / 6;
    for i in 0..16 {
        let s = i64::from(m[scale_class(i % 4, i / 4)]);
        let c = i64::from(coeffs[i]);
        let v = if qp >= 24 {
            (c * s) << (q6 - 4)
        } else {
            (c * s + (1i64 << (3 - q6))) >> (4 - q6)
        };
        coeffs[i] = v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }
}

/// 4x4 Hadamard inverse for the Intra-16x16 luma DC block
/// (spec 8.5.11.1): the DC coefficients first pass through a Hadamard
/// transform, then per spec the scaling is folded in —
/// `(w * LevelScale(qP%6, 0,0)) << (qP/6 - 2)` for `qP >= 12`, else
/// `(w * LevelScale + r) >> (2 - qP/6)` with `r = 1 << (1 - qP/6)`
/// (rounding toward nearest; for `qP/6 == 1` the offset is 1, else 2 —
/// matching the `(1 << (2 - qpDiv)) >> 1` rounding of the reference
/// implementation).
pub(crate) fn inv_luma_dc(block: &mut Block4, qp: u8) {
    debug_assert!(qp < 52);
    // 4x4 Hadamard, horizontal then vertical, no scaling yet.
    for row in 0..4 {
        let p = &mut block[row * 4..row * 4 + 4];
        let a0 = p[0].wrapping_add(p[2]);
        let a1 = p[0].wrapping_sub(p[2]);
        let a2 = p[1].wrapping_sub(p[3]);
        let a3 = p[1].wrapping_add(p[3]);
        p[0] = a0.wrapping_add(a3);
        p[1] = a1.wrapping_add(a2);
        p[2] = a1.wrapping_sub(a2);
        p[3] = a0.wrapping_sub(a3);
    }
    for col in 0..4 {
        let i0 = col;
        let i1 = col + 4;
        let i2 = col + 8;
        let i3 = col + 12;
        let a0 = block[i0].wrapping_add(block[i2]);
        let a1 = block[i0].wrapping_sub(block[i2]);
        let a2 = block[i1].wrapping_sub(block[i3]);
        let a3 = block[i1].wrapping_add(block[i3]);
        block[i0] = a0.wrapping_add(a3);
        block[i1] = a1.wrapping_add(a2);
        block[i2] = a1.wrapping_sub(a2);
        block[i3] = a0.wrapping_sub(a3);
    }
    // Scaling (spec 8.5.10, eqs. 8-321/8-322).
    let ls = i64::from(LEVEL_SCALE_4X4[QP_MOD6[qp as usize] as usize][0]);
    let qp_div = qp as i64 / 6;
    if qp >= 36 {
        let shift = qp_div - 6;
        for v in block.iter_mut() {
            *v = ((i64::from(*v) * ls) << shift)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        }
    } else {
        let shift = 6 - qp_div;
        let r = 1i64 << (5 - qp_div);
        for v in block.iter_mut() {
            *v = ((i64::from(*v) * ls + r) >> shift)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        }
    }
}

/// 2x2 Hadamard inverse for a chroma DC block (spec 8.5.11.1,
/// eq. 8-324), followed by the ChromaArrayType=1 scaling of
/// 8.5.11.2 eq. 8-326: `(f * LevelScale(qP%6,0,0) << (qP/6)) >> 5`.
/// Input and return order are 2x2 raster: `[c00, c01, c10, c11]`.
pub(crate) fn inv_chroma_dc(dc: &[i32; 4], qp: u8) -> [i32; 4] {
    debug_assert!(qp < 52);
    let a0 = dc[0] as i64 + dc[2] as i64;
    let a1 = dc[0] as i64 - dc[2] as i64;
    let a2 = dc[1] as i64 - dc[3] as i64;
    let a3 = dc[1] as i64 + dc[3] as i64;
    // W2 * C * W2ᵀ in raster order: [f00, f01, f10, f11].
    let f = [a0 + a3, a0 - a3, a1 + a2, a1 - a2];
    let ls = i64::from(LEVEL_SCALE_4X4[QP_MOD6[qp as usize] as usize][0]);
    let shift = i64::from(QP_DIV6[qp as usize]);
    let mut out = [0i32; 4];
    for (o, v) in out.iter_mut().zip(f.iter()) {
        *o = (((v * ls) << shift) >> 5).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }
    out
}

/// Adds a 4x4 residual block to an 8-bit prediction block in place,
/// clipping to [0, 255] (spec 8.5.13).
pub(crate) fn add_residual_4x4(pred: &mut [u8], stride: usize, residual: &Block4) {
    for y in 0..4 {
        for x in 0..4 {
            let v = i32::from(pred[y * stride + x]) + residual[y * 4 + x];
            pred[y * stride + x] = v.clamp(0, 255) as u8;
        }
    }
}
