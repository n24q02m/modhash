//! Raw image buffers, box-average resampling, BT.601 luma and EXIF orientation.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! This crate is the shared destination every image decoder in the kit
//! writes into, and the shared source every image hash reads from. The
//! conventions that decide pixel values — tight row-major layout, integer
//! box boundaries, `round_half_up` everywhere, the EXIF orientation table —
//! are specified in `docs/algorithms/raster.md` and pinned by the tests in
//! `tests/`; an implementation that silently rounds differently produces
//! near-miss hashes, so the rules live here exactly once.
//!
//! The crate is `no_std` apart from the `alloc` [`Vec`](alloc::vec::Vec)
//! the buffers own, and every allocation is capped by
//! [`MAX_BUFFER_BYTES`](crate::raster::MAX_BUFFER_BYTES) before a byte is
//! reserved.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod color;
mod exif;
mod raster;
mod resample;

pub use crate::color::luma_bt601;
pub use crate::exif::apply_orientation;
pub use crate::raster::{Gray, Image, Layout, MAX_BUFFER_BYTES, Rgb, Rgba, Sample};
pub use crate::resample::box_average;
