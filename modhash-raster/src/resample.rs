//! Box-average resampling: the only resizer the kit needs.
//!
//! Output pixel `(x, y)` averages the axis-aligned source rectangle that
//! maps to it under integer floor boundaries — column `x` covers
//! `⌊x·W/W'⌋ .. ⌊(x+1)·W/W'⌋` and row `y` likewise. The boundaries are
//! the spec's accumulated-remainder rule made exact: box widths differ by
//! at most one source pixel and are fully determined by the four
//! dimensions, so two independent implementations bit-match.
//!
//! Each output sample is `round_half_up(mean)` of its box, computed
//! entirely in `u64` — `(2·sum + n) / (2·n)`. No fractional coefficients,
//! no float, one rounding step. A separable two-pass filter would round
//! twice and drift off the exact box mean; the one-pass form is both
//! simpler and exact, and `box_average(im, 1, 1)` is the exact mean of
//! the whole buffer.
//!
//! Upscaling is defined but not what the kit does: when a box is empty
//! (`out > in` on an axis), the pixel samples the nearest source point —
//! the box centre `⌊(2j+1)·in/(2·out)⌋`. Documented so the function is
//! total, not offered as a resampling feature.
use alloc::vec;

use crate::raster::{Image, Layout, Sample};
use modhash_primitives::{Error, Result};

/// Inclusive-exclusive source index range of output slot `j` along one
/// axis: `[⌊j·n/n'⌋, ⌊(j+1)·n/n'⌋)` clamped to `n`. When the range is
/// empty — upscaling makes `⌊(j+1)·n/n'⌋ == ⌊j·n/n'⌋` — it collapses to
/// the single nearest source index, the box centre
/// `⌊(2j+1)·n/(2·n')⌋`, which is at most `n−1`.
#[inline]
fn box_range(j: u32, n_in: u32, n_out: u32) -> (u32, u32) {
    let lo = (u64::from(j) * u64::from(n_in) / u64::from(n_out)) as u32;
    let hi = (u64::from(j + 1) * u64::from(n_in) / u64::from(n_out)) as u32;
    if lo < hi {
        return (lo, hi);
    }
    let c = ((u64::from(2 * j + 1) * u64::from(n_in)) / (2 * u64::from(n_out))) as u32;
    (c.min(n_in - 1), c.min(n_in - 1) + 1)
}

/// Resizes `img` to `out_w`×`out_h` by integer box averaging.
///
/// `Error::BadValue` on a zero output dimension. Layout, sample type and
/// channel count pass through unchanged; `Image`'s own construction
/// rules keep the result inside [`crate::MAX_BUFFER_BYTES`].
pub fn box_average<L: Layout, T: Sample>(
    img: &Image<L, T>,
    out_w: u32,
    out_h: u32,
) -> Result<Image<L, T>> {
    if out_w == 0 || out_h == 0 {
        return Err(Error::BadValue("output dimension is zero"));
    }
    let c = L::CHANNELS;
    let (in_w, in_h) = (img.width(), img.height());
    let src = img.as_slice();
    let mut out = vec![T::default(); out_w as usize * out_h as usize * c];

    for y in 0..out_h {
        let (y0, y1) = box_range(y, in_h, out_h);
        for x in 0..out_w {
            let (x0, x1) = box_range(x, in_w, out_w);
            // Box is at least one sample on each axis by construction.
            let n = u64::from(y1 - y0) * u64::from(x1 - x0);
            let base = (y as usize) * out_w as usize * c + x as usize * c;
            for (k, slot) in out[base..base + c].iter_mut().enumerate() {
                let mut sum = 0u64;
                for sy in y0..y1 {
                    let row = (sy as usize) * in_w as usize * c;
                    for sx in x0..x1 {
                        sum += src[row + sx as usize * c + k].to_u64();
                    }
                }
                *slot = T::from_u64((2 * sum + n) / (2 * n));
            }
        }
    }
    // from_vec cannot fail: dimensions are nonzero, the buffer is exact,
    // and out_w*out_h was already allocated so it fits the byte cap.
    Image::from_vec(out_w, out_h, out)
}
