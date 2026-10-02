//! The shared vocabulary of the `modhash` kit: the one error type, the
//! fixed-width digest value, the Hamming distance between digests, and
//! the closed sets of names that appear in on-disk records.
//!
//! Crate `#0` of the zero-dependency `modhash` workspace. Every other
//! crate builds on these types and nothing builds beneath them: this
//! crate has an empty `[dependencies]` table and no workspace
//! dependencies, enforced by `scripts/gate_zero_dep.sh` and
//! `scripts/gate_dag.py`.
//!
//! The crate is `core`-only: no `std`, no allocator, no filesystem,
//! network, clock or environment. Nothing here makes a result depend on
//! anything but its arguments, and no code path allocates.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod digest;
mod distance;
mod error;
mod names;

pub use crate::digest::{Digest, Read};
pub use crate::distance::hamming;
pub use crate::error::{Error, Result};
pub use crate::names::{Algorithm, Format};
