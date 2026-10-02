//! The 64-bit image perceptual hash (`docs/algorithms/kit.md` §3).
//!
//! The chain is fixed and each step is one call into a lower crate, so
//! this file is the *order* of the pipeline, not a reimplementation of
//! any step:
//!
//! `to u8 → luma_bt601 → box_average 32×32 → dct2_2d → low 8×8,
//! drop DC → threshold = median of the remaining 63 → 1 bit per
//! coefficient`.
//!
//! Bit order: row-major over the kept 8×8 block, coefficient `(y, x)`
//! sets bit `63 − (8y + x)` when it exceeds the median — MSB is the DC
//! corner, LSB is `[7][7]`. The DC coefficient is compared against the
//! same median but never joins the 63-value pool it is thresholded by.
//!
//! The oracle is `lab/phash_oracle.py` — an independent Python
//! implementation whose output is committed to
//! `tests/phash_vectors.rs`; a bit of drift in any step (rounding,
//! median convention, bit order, DC handling) flips committed bits and
//! turns the fixture test red.

use crate::{Error, Modality};
use modhash_math::{dct2_2d, median};
use modhash_raster::{Gray, Image, Layout, Sample, box_average, luma_bt601};

/// The square the DCT runs on.
const DCT: usize = 32;
/// The low-frequency block the hash keeps.
const KEEP: usize = 8;

/// Every fallible step maps to this error with the step named. The
/// construction rules of [`Image`] make these unreachable in practice —
/// dimensions are nonzero and buffers are exact — but a function that
/// cannot panic still *states* its refusal instead of relying on an
/// invariant the signature doesn't carry.
fn step_err(what: &'static str) -> Error {
    Error::Decode {
        modality: Modality::Image,
        source: modhash_primitives::Error::BadValue(what),
    }
}

/// Reduces one sample of any depth to `u8`: the identity for `u8`,
/// `v >> 8` for `u16` (PNG's sanctioned 16→8 reduction — drop the low
/// byte, as `docs/algorithms/kit.md` §2 pins).
#[inline]
fn to_u8<T: Sample>(v: T) -> u8 {
    if T::MAX_U64 == 255 {
        v.to_u64() as u8
    } else {
        (v.to_u64() >> 8) as u8
    }
}

/// The 64-bit perceptual hash of `img`.
///
/// Any layout works: `Gray` pixels are already luma, `Rgb`/`Rgba`
/// convert through BT.601 with alpha ignored; `u16` samples reduce to
/// `u8` first so the luma arithmetic is the spec's `[0,255]` domain.
///
/// # Errors
///
/// [`Error::Decode`] with `modality: image` naming the failed step.
/// With a well-formed [`Image`] this never happens — the errors exist
/// because the function takes untrusted-shaped data through code whose
/// contract is "no panics, ever".
pub fn image_phash<L: Layout, T: Sample>(img: &Image<L, T>) -> Result<u64, Error> {
    // Step 1+2: luma into a Gray u8 image at source size, then the
    // integer box average onto the 32×32 analysis grid.
    let (w, h) = (img.width() as usize, img.height() as usize);
    let src = img.as_slice();
    let c = L::CHANNELS;
    let mut gray = Image::<Gray, u8>::new(img.width(), img.height())
        .map_err(|_| step_err("phash luma buffer"))?;
    {
        let out = gray.as_mut_slice();
        if out.len() < w * h || src.len() < w * h * c {
            return Err(step_err("phash image buffer undersized"));
        }
        for i in 0..w * h {
            let p = &src[i * c..i * c + c];
            out[i] = match c {
                1 => to_u8(p[0]),
                _ => luma_bt601(to_u8(p[0]), to_u8(p[1]), to_u8(p[2])),
            };
        }
    }
    let small = box_average(&gray, DCT as u32, DCT as u32)
        .map_err(|_| step_err("phash 32x32 box average"))?;

    // Step 3: orthonormal DCT-II, rows then columns, over the 32×32
    // block as f64.
    let mut block = [0.0f64; DCT * DCT];
    for (dst, v) in block.iter_mut().zip(small.as_slice()) {
        *dst = f64::from(*v);
    }
    dct2_2d(&mut block, DCT, DCT);

    // Step 4+5: keep the low 8×8; the threshold is the lower-middle
    // median of the 63 coefficients that are not the DC term.
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
    let t = median(&mut rest).ok_or_else(|| step_err("phash median of 63"))?;

    // Step 6: one bit per coefficient, MSB = [0][0].
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
