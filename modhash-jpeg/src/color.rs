//! Color conversion: component planes → output image.
//!
//! YCbCr→RGB uses libjpeg's fixed-point formulation (jdcolor.c):
//! SCALEBITS = 16 tables built exactly as `build_ycc_rgb_table`, then
//! `r = clamp(y + crr[cr])` etc. This is *not* an arbitrary choice of
//! coefficient — the rounding and truncation of `FIX(1.40200)` et al.
//! are pinned so the result is byte-exact with the reference decoder.
//!
//! Color space selection mirrors libjpeg's inference rules: an Adobe
//! APP14 `transform` flag wins (0 = RGB passthrough), otherwise JFIF
//! (APP0) implies YCbCr; a bare stream with component ids 82/71/66
//! ('R','G','B') is RGB, anything else is treated as YCbCr — matching
//! the pragmatic behavior of every common decoder.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};
use modhash_raster::{Gray, Image, Rgb};

use crate::Jpeg;
use crate::parser::Frame;

const SCALEBITS: i64 = 16;
const ONE_HALF: i64 = 1 << (SCALEBITS - 1);
const CENTER: i64 = 128;
/// `FIX(x)` for the color constants.
const fn fix(x_num: i64, x_den: i64) -> i64 {
    (x_num * (1 << SCALEBITS) + x_den / 2) / x_den
}

const FIX_1_40200: i64 = fix(140200, 100000);
const FIX_1_77200: i64 = fix(177200, 100000);
const FIX_0_71414: i64 = fix(71414, 100000);
const FIX_0_34414: i64 = fix(34414, 100000);

#[inline]
fn clamp8(v: i64) -> u8 {
    if v < 0 {
        0
    } else if v > 255 {
        255
    } else {
        v as u8
    }
}

/// Assembles the final [`Jpeg`] from full-resolution component planes.
///
/// `planes[i]` is `width × height` u8. `adobe` is the APP14 transform
/// flag when present.
pub(crate) fn convert(frame: &Frame, planes: &[Vec<u8>], adobe: Option<u8>) -> Result<Jpeg> {
    let w = frame.width as u32;
    let h = frame.height as u32;
    let n = frame.comps.len();

    if n == 1 {
        let img = Image::<Gray, u8>::from_vec(w, h, planes[0].clone())?;
        return Ok(Jpeg::Gray(img));
    }
    if n != 3 {
        return Err(Error::Unsupported("component count outside 1 or 3"));
    }

    // Decide color space.
    let is_rgb = match adobe {
        Some(0) => true,
        Some(1) => false,
        Some(_) => return Err(Error::Unsupported("Adobe transform flag 2 (YCCK)")),
        None => {
            let ids: Vec<u8> = frame.comps.iter().map(|c| c.id).collect();
            ids == [b'R', b'G', b'B']
        }
    };

    let (y, cb, cr) = (&planes[0], &planes[1], &planes[2]);
    let npix = frame.width * frame.height;
    let mut rgb = alloc::vec![0u8; npix * 3];

    if is_rgb {
        for i in 0..npix {
            rgb[i * 3] = y[i];
            rgb[i * 3 + 1] = cb[i];
            rgb[i * 3 + 2] = cr[i];
        }
    } else {
        // Per-pixel fixed-point YCbCr→RGB, identical arithmetic to
        // libjpeg's table lookups (the tables are precomputed per Cr/Cb
        // value; here the expressions are written inline).
        for i in 0..npix {
            let yv = i64::from(y[i]);
            let cbv = i64::from(cb[i]);
            let crv = i64::from(cr[i]);
            // Cr_r = RIGHT_SHIFT(FIX(1.40200)*(cr-128) + HALF, 16)
            let d_cr = crv - CENTER;
            let d_cb = cbv - CENTER;
            let r = yv + ((FIX_1_40200 * d_cr + ONE_HALF) >> SCALEBITS);
            let g = yv + ((-FIX_0_34414 * d_cb - FIX_0_71414 * d_cr + ONE_HALF) >> SCALEBITS);
            let b = yv + ((FIX_1_77200 * d_cb + ONE_HALF) >> SCALEBITS);
            rgb[i * 3] = clamp8(r);
            rgb[i * 3 + 1] = clamp8(g);
            rgb[i * 3 + 2] = clamp8(b);
        }
    }

    let img =
        Image::<Rgb, u8>::from_vec(w, h, rgb).map_err(|_| Error::BadValue("rgb image buffer"))?;
    Ok(Jpeg::Rgb(img))
}
