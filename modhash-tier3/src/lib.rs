//! Tier-3 local features of the `modhash` zero-dependency hashing kit:
//! FAST-9 corner detection, rotation-aware steered-BRIEF (rBRIEF)
//! descriptors, deterministic RANSAC geometric verification, the eight
//! dihedral (D₄) image symmetries, and banded dynamic time warping —
//! the "cổ điển cục bộ" (classical, non-ML) layer of spec §4.3.
//!
//! Part of the `modhash` zero-dependency hashing kit: this crate
//! depends only on the kit's own `modhash-math` (the `solve3` linear
//! solve the RANSAC triplet fit needs) and `modhash-raster` (the
//! `Image` buffers everything operates on). `scripts/gate_dag.py`
//! declares exactly those two edges — which is why this crate defines
//! its own small [`Error`] type and keeps a private copy of the
//! splitmix64 recurrence ([`BRIEF_PAIRS`](orb::BRIEF_PAIRS) and the
//! RANSAC sampler) rather than reaching for `modhash-primitives`.
//!
//! The crate is `std`, not `no_std`: rBRIEF patch orientation and the
//! RANSAC rotation recovery need `f64::atan2`/`sin`/`cos`, which `core`
//! does not provide — the same reason `modhash-math` is `std`.
//!
//! # The pinned conventions (spec §4.3)
//!
//! * **FAST-9**: 16-pixel radius-3 circle, contrast threshold 20,
//!   9 contiguous pixels strictly beyond it ([`orb::fast9`]).
//! * **rBRIEF**: 31×31 patch, intensity-centroid orientation,
//!   256 fixed test pairs inside the radius-15 disk, MSB-first
//!   bitpacking ([`orb::rbrief`]).
//! * **RANSAC**: 3-point affine solve, 4 px inlier threshold, 500
//!   deterministic seeded draws ([`ransac::ransac_affine`]).
//! * **D₄**: the square's eight symmetries, square input or a centred
//!   square crop ([`d4::transform`], [`d4::center_square`]).
//! * **DTW**: per-step mean cost, Sakoe–Chiba band `w = 10 %` with a
//!   2-cell floor, `O(n·w)` time / `O(m)` space ([`dtw::dtw`]).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod d4;
pub mod dtw;
pub mod orb;
mod prng;
pub mod ransac;

/// The crate's single error type: every refusal is one of these.
///
/// Deliberately small — tier-3 math has no "corrupt stream" concept,
/// only invalid shapes and genuinely unsolvable sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// An input violated a documented precondition (zero dimension,
    /// mismatched lengths, non-square image for a D₄ transform, cost
    /// matrix whose length is not `n*m`).
    BadValue(&'static str),
    /// RANSAC drew only degenerate triplets for the whole iteration
    /// budget — no model exists to return.
    NoFit,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::BadValue(why) => write!(f, "bad value: {why}"),
            Error::NoFit => write!(f, "no RANSAC model survived sampling"),
        }
    }
}

impl std::error::Error for Error {}
