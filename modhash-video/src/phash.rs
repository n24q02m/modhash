//! The per-frame perceptual hash — the image pipeline of
//! `docs/algorithms/kit.md` §3 applied to a decoded luma plane.
//!
//! The chain is identical to `modhash`'s `phash` module, step for step:
//! `luma plane → box_average 32×32 → orthonormal dct2_2d → low 8×8, drop
//! DC → threshold = lower-middle median of the remaining 63 → 1 bit per
//! coefficient, MSB = [0][0]`. Two pieces are reimplemented here rather
//! than imported because the workspace DAG (`scripts/gate_dag.py`
//! EXPECTED_EDGES) deliberately gives `modhash-video` no `modhash-math`
//! edge: the orthonormal DCT-II kernel and the lower-middle median are
//! copied *verbatim* in semantics — same summation order, same lower-middle
//! pick — so a frame hashed here and the same frame run through
//! `modhash::image_phash` produce the identical `u64` (pinned by test).

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use modhash_primitives::Result;
use modhash_raster::{Gray, Image, box_average};

/// The square the DCT runs on (kit.md §3, step 2).
const DCT: usize = 32;
/// The low-frequency block the hash keeps (kit.md §3, step 4).
const KEEP: usize = 8;

/// `modhash_math::dct2_2d`'s kernel, reimplemented: orthonormal DCT-II,
/// `F[k] = c_k·√(2/N)·Σ_n x[n]·cos(π·(2n+1)·k/(2N))`, applied rows then
/// columns. The direct O(N²) sum *is* the definition; summation order and
/// scaling match `modhash-math/src/dct.rs` exactly so results are
/// bit-identical.
fn dct2_2d(block: &mut [f64; DCT * DCT]) {
    let n = DCT;
    let scale = (2.0 / n as f64).sqrt();
    let kernel = |x: &[f64], out: &mut [f64]| {
        for (k, slot) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (i, &xi) in x.iter().enumerate() {
                acc += xi * (PI * ((2 * i + 1) * k) as f64 / (2.0 * n as f64)).cos();
            }
            let c_k = if k == 0 { FRAC_1_SQRT_2 } else { 1.0 };
            *slot = c_k * scale * acc;
        }
    };
    let mut row_out = [0.0f64; DCT];
    for row in block.chunks_exact_mut(DCT) {
        kernel(row, &mut row_out);
        row.copy_from_slice(&row_out);
    }
    let mut gather = [0.0f64; DCT];
    let mut col_out = [0.0f64; DCT];
    for x in 0..DCT {
        for (y, g) in gather.iter_mut().enumerate() {
            *g = block[y * DCT + x];
        }
        kernel(&gather, &mut col_out);
        for (y, g) in col_out.iter().enumerate() {
            block[y * DCT + x] = *g;
        }
    }
}

/// `modhash_math::median`'s lower-middle convention on a 63-element
/// block: `sorted[(n−1)/2]` after a `f64::total_cmp` sort — never an
/// average of the middle pair (see `modhash-math/src/median.rs` for why
/// the threshold must stay a member of the input set).
fn median63(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[(v.len() - 1) / 2]
}

/// The 64-bit perceptual hash of one decoded frame's luma plane.
///
/// `luma` must be exactly `width * height` bytes, row-major, `width` and
/// `height` nonzero — the [`modhash_h264::Frame`] `y` plane satisfies all
/// three by construction; other callers get [`modhash_primitives::Error::BadValue`]
/// on a shape mismatch rather than a slice panic.
///
/// # Errors
///
/// [`modhash_primitives::Error::BadValue`] on a zero dimension or an
/// undersized plane.
pub fn frame_phash(width: u32, height: u32, luma: &[u8]) -> Result<u64> {
    let need = width as usize * height as usize;
    if width == 0 || height == 0 {
        return Err(modhash_primitives::Error::BadValue(
            "video frame has zero dimension",
        ));
    }
    if luma.len() < need {
        return Err(modhash_primitives::Error::BadValue(
            "video luma plane smaller than width*height",
        ));
    }
    // from_vec cannot fail: dimensions are nonzero and the slice length
    // was just checked to be exact. `get(..need)` keeps a caller's
    // over-long buffer from making the shape check a lie.
    let gray = Image::<Gray, u8>::from_vec(width, height, luma[..need].to_vec())?;
    let small = box_average(&gray, DCT as u32, DCT as u32)?;

    // Orthonormal DCT-II over the 32×32 block as f64 (kit.md §3 step 3).
    let mut block = [0.0f64; DCT * DCT];
    for (dst, v) in block.iter_mut().zip(small.as_slice()) {
        *dst = f64::from(*v);
    }
    dct2_2d(&mut block);

    // Low 8×8, threshold = lower-middle median of the 63 non-DC terms.
    let kept = |x: usize, y: usize| block[y * DCT + x];
    let mut rest = [0.0f64; KEEP * KEEP - 1];
    let mut n = 0;
    for y in 0..KEEP {
        for x in 0..KEEP {
            if x == 0 && y == 0 {
                continue;
            }
            rest[n] = kept(x, y);
            n += 1;
        }
    }
    let t = median63(&mut rest);

    // One bit per coefficient, MSB = [0][0]; DC is compared against the
    // same median but never joined the pool that produced it.
    let mut out = 0u64;
    for y in 0..KEEP {
        for x in 0..KEEP {
            if kept(x, y) > t {
                out |= 1u64 << (63 - (y * KEEP + x));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use modhash_primitives::SplitMix64;

    #[test]
    fn rejects_bad_shapes() {
        assert!(frame_phash(0, 4, &[0; 64]).is_err());
        assert!(frame_phash(4, 0, &[0; 64]).is_err());
        assert!(frame_phash(8, 8, &[0; 63]).is_err());
        // Over-long buffers are sliced to the exact plane, not refused.
        assert!(frame_phash(8, 8, &[7; 100]).is_ok());
    }

    #[test]
    fn flat_and_gradient_planes_hash() {
        // A flat frame's DCT is DC-only: every coefficient except [0][0]
        // is ~0 and none exceeds the median-of-63 — the hash depends on
        // the DC bit alone and is stable under the exact math.
        let flat = frame_phash(16, 16, &[128u8; 256]).unwrap();
        let flat2 = frame_phash(16, 16, &[128u8; 256]).unwrap();
        assert_eq!(flat, flat2);
        // A gradient must differ from a flat field.
        let mut grad = [0u8; 256];
        for y in 0..16 {
            for x in 0..16 {
                grad[y * 16 + x] = ((x * 16 + y) % 256) as u8;
            }
        }
        assert_ne!(frame_phash(16, 16, &grad).unwrap(), flat);
    }

    #[test]
    fn deterministic_on_noise() {
        let mut rng = SplitMix64::new(0xFACE);
        let mut plane = [0u8; 64 * 48];
        for b in &mut plane {
            *b = rng.next_u64() as u8;
        }
        assert_eq!(
            frame_phash(64, 48, &plane).unwrap(),
            frame_phash(64, 48, &plane).unwrap()
        );
    }
}
