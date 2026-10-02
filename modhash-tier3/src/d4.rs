//! The eight symmetries of the square — the dihedral group D₄ (order
//! 8: four rotations composed with the optional main-diagonal
//! reflection) — applied to square images per spec §4.3: *"8 phép biến
//! (4 xoay × 2 phản chiếu), chỉ khi ảnh vuông hoặc center-crop vuông"*.
//!
//! Every transform is an exact pixel permutation delegated to
//! [`modhash_raster::Image`]'s own `rot*`/`transpose` methods, so the
//! mapping rules (clockwise rotations, main-diagonal transpose) are the
//! raster crate's single convention and cannot drift here.
//!
//! D₄ only closes on squares: rotating a `w×h` rectangle with `w ≠ h`
//! changes its shape, so [`transform`] and [`variants`] require square
//! input. [`center_square`] supplies the spec's escape hatch —
//! non-square images lose their longest side's margin, deterministically
//! (extra row/column counts split evenly, the odd pixel landing in the
//! leading margin).
//!
//! Orientation coverage complements rBRIEF steering: the descriptor is
//! rotation-aware but not reflection-aware, so the eight variants let a
//! caller compare a flipped image without re-deriving the geometry.

use alloc::vec::Vec;

use modhash_raster::{Image, Layout, Sample};

use crate::Error;

/// One element of D₄, the symmetry group of the square.
///
/// `Reflect` is the transpose (mirror across the main diagonal,
/// top-left fixed); every element is `RotK` optionally followed by the
/// reflection. The eight `all()` elements enumerate the group exactly
/// once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum D4 {
    /// Identity.
    Rot0,
    /// 90° clockwise.
    Rot90,
    /// 180°.
    Rot180,
    /// 270° clockwise (90° counter-clockwise).
    Rot270,
    /// Transpose (main-diagonal reflection).
    Transpose,
    /// Transpose then 90° clockwise.
    TransposeRot90,
    /// Transpose then 180°.
    TransposeRot180,
    /// Transpose then 270° clockwise — the anti-diagonal reflection.
    TransposeRot270,
}

impl D4 {
    /// All eight group elements, in a fixed enumeration order.
    pub fn all() -> [D4; 8] {
        [
            D4::Rot0,
            D4::Rot90,
            D4::Rot180,
            D4::Rot270,
            D4::Transpose,
            D4::TransposeRot90,
            D4::TransposeRot180,
            D4::TransposeRot270,
        ]
    }
}

/// Apply one D₄ element to a **square** image.
///
/// `Error::BadValue` on non-square input — the group has no
/// shape-preserving action on rectangles, and silently cropping inside
/// a "transform" would hide a caller mistake.
pub fn transform<L: Layout, T: Sample>(img: &Image<L, T>, op: D4) -> Result<Image<L, T>, Error> {
    if img.width() != img.height() {
        return Err(Error::BadValue("D4 transform requires a square image"));
    }
    let out = match op {
        D4::Rot0 => img.clone(),
        D4::Rot90 => img.rot90(),
        D4::Rot180 => img.rot180(),
        D4::Rot270 => img.rot270(),
        D4::Transpose => img.transpose(),
        D4::TransposeRot90 => img.transpose().rot90(),
        D4::TransposeRot180 => img.transpose().rot180(),
        D4::TransposeRot270 => img.transpose().rot270(),
    };
    Ok(out)
}

/// The eight D₄ variants of a square image, in [`D4::all`] order.
pub fn variants<L: Layout, T: Sample>(img: &Image<L, T>) -> Result<Vec<Image<L, T>>, Error> {
    if img.width() != img.height() {
        return Err(Error::BadValue("D4 variants require a square image"));
    }
    Ok(D4::all()
        .iter()
        .map(|&op| transform(img, op).expect("square checked"))
        .collect())
}

/// Crop the centred `min(w, h)²` square out of `img` — the spec's
/// conditioning step for feeding non-square input to [`transform`].
///
/// The margin splits evenly between the two ends; when `w − min` or
/// `h − min` is odd, the extra pixel goes to the leading (left/top)
/// margin — deterministic on every platform.
pub fn center_square<L: Layout, T: Sample>(img: &Image<L, T>) -> Result<Image<L, T>, Error> {
    let side = img.width().min(img.height());
    let x0 = (img.width() - side) / 2;
    let y0 = (img.height() - side) / 2;
    let c = L::CHANNELS;
    let (w, side, x0, y0) = (
        img.width() as usize,
        side as usize,
        x0 as usize,
        y0 as usize,
    );
    let mut out = Vec::with_capacity(side * side * c);
    for y in 0..side {
        let row = (y0 + y) * w * c + x0 * c;
        out.extend_from_slice(&img.as_slice()[row..row + side * c]);
    }
    Image::from_vec(
        img.width().min(img.height()),
        img.width().min(img.height()),
        out,
    )
    .map_err(|_| Error::BadValue("center-square crop rejected"))
}
