//! BT.601 luma: `Y = round_half_up(0.299·R + 0.587·G + 0.114·B)`.
//!
//! The spec fixes the formula and the rounding; this module fixes the
//! arithmetic. `(299·R + 587·G + 114·B + 500) / 1000` in `u64` **is**
//! `round_half_up(0.299·R + 0.587·G + 0.114·B)` exactly — the decimal
//! coefficients are rationals with denominator 1000, so integer division
//! computes the mathematical value with no float representation error.
//! A float `0.299` is not exactly `299/1000`; that difference is what
//! this form eliminates.
//!
//! Half-up (`.5` away from zero) is the kit's convention: `round_half_up`
//! everywhere, pinned by tests. The result is clamped to the sample
//! range defensively, though a sum of three weights totalling exactly 1
//! over in-range inputs can never exceed it.
//!
//! `Image<Rgb>` converts per pixel through [`luma_bt601`]; `Image<Rgba>`
//! ignores the alpha channel outright — luma of the picture is wanted,
//! not a premultiplied composite, which is a rendering concern the
//! hashing pipeline does not have.

use alloc::vec::Vec;

use crate::raster::{Gray, Image, Rgb, Rgba, Sample};

/// BT.601 luma of one pixel: `round_half_up(0.299·r + 0.587·g +
/// 0.114·b)`, clamped to the sample's range.
///
/// Exact integer form: `round_half_up((299·r + 587·g + 114·b) / 1000)`.
/// Works for `u8` and `u16` samples; intermediates never leave `u64`.
#[inline]
pub fn luma_bt601<T: Sample>(r: T, g: T, b: T) -> T {
    let sum = 299 * r.to_u64() + 587 * g.to_u64() + 114 * b.to_u64();
    let y = (sum + 500) / 1000;
    T::from_u64(y.min(T::MAX_U64))
}

impl Image<Rgb, u8> {
    /// Per-pixel BT.601 luma as a `Gray` image of the same dimensions.
    pub fn to_gray(&self) -> Image<Gray, u8> {
        to_gray_rgb(self)
    }
}

impl Image<Rgb, u16> {
    /// Per-pixel BT.601 luma as a `Gray` image of the same dimensions.
    pub fn to_gray(&self) -> Image<Gray, u16> {
        to_gray_rgb(self)
    }
}

impl Image<Rgba, u8> {
    /// Per-pixel BT.601 luma of the RGB channels; **alpha is ignored**,
    /// not composited.
    pub fn to_gray(&self) -> Image<Gray, u8> {
        to_gray_rgba(self)
    }
}

impl Image<Rgba, u16> {
    /// Per-pixel BT.601 luma of the RGB channels; **alpha is ignored**,
    /// not composited.
    pub fn to_gray(&self) -> Image<Gray, u16> {
        to_gray_rgba(self)
    }
}

fn to_gray_rgb<T: Sample>(img: &Image<Rgb, T>) -> Image<Gray, T> {
    let mut out = Vec::with_capacity((img.width() * img.height()) as usize);
    for px in img.as_slice().chunks_exact(3) {
        out.push(luma_bt601(px[0], px[1], px[2]));
    }
    // Cannot fail: same pixel count as the source, one channel.
    match Image::from_vec(img.width(), img.height(), out) {
        Ok(im) => im,
        Err(_) => unreachable!("luma buffer is exactly width*height samples"),
    }
}

fn to_gray_rgba<T: Sample>(img: &Image<Rgba, T>) -> Image<Gray, T> {
    let mut out = Vec::with_capacity((img.width() * img.height()) as usize);
    for px in img.as_slice().chunks_exact(4) {
        out.push(luma_bt601(px[0], px[1], px[2]));
    }
    match Image::from_vec(img.width(), img.height(), out) {
        Ok(im) => im,
        Err(_) => unreachable!("luma buffer is exactly width*height samples"),
    }
}
