//! EXIF orientation application.
//!
//! A stored raster is the decode order; the orientation tag says which
//! dihedral transform restores display order. This module applies the
//! tag — nothing more of EXIF is parsed here (the tag value arrives
//! already decoded by the codec that found it).
//!
//! Tag → transform table, `w`/`h` the stored dimensions:
//!
//! | tag | operation                     | output dims |
//! |-----|-------------------------------|-------------|
//! | 1   | identity (top-left origin)    | w × h       |
//! | 2   | [`flip_h`](crate::Image::flip_h)      | w × h |
//! | 3   | [`rot180`](crate::Image::rot180)      | w × h |
//! | 4   | [`flip_v`](crate::Image::flip_v)      | w × h |
//! | 5   | [`transpose`](crate::Image::transpose)  | h × w |
//! | 6   | [`rot90`](crate::Image::rot90) clockwise | h × w |
//! | 7   | [`transverse`](crate::Image::transverse) | h × w |
//! | 8   | [`rot270`](crate::Image::rot270) clockwise | h × w |
//!
//! The eight transforms are the dihedral group D4: every composition is
//! another group element, so `rot90(rot90(im))` is `rot180(im)` and
//! `rot90(rot270(im))` is the identity. Tests pin each tag's exact
//! output layout plus those compositions.

use crate::raster::{Image, Layout, Sample};
use modhash_primitives::{Error, Result};

/// Applies EXIF orientation `tag` (`1..=8`) to `img`, returning the
/// image in display order.
///
/// `Error::BadValue("exif orientation")` for any other value, including
/// the `0` a missing tag sometimes surfaces as: a decoder that cannot
/// prove the orientation must not have the buffer silently rotated.
pub fn apply_orientation<L: Layout, T: Sample>(img: &Image<L, T>, tag: u8) -> Result<Image<L, T>> {
    let out = match tag {
        1 => img.clone(),
        2 => img.flip_h(),
        3 => img.rot180(),
        4 => img.flip_v(),
        5 => img.transpose(),
        6 => img.rot90(),
        7 => img.transverse(),
        8 => img.rot270(),
        _ => return Err(Error::BadValue("exif orientation")),
    };
    Ok(out)
}
