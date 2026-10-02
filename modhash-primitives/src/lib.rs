//! The shared vocabulary of the `modhash` kit: the one error type, the
//! fixed-width digest value, the Hamming distance between digests, and
//! the closed sets of names that appear in on-disk records. Also the
//! computational primitives every modality crate shares: SHA-256, the
//! checksums (CRC-32, Adler-32, FNV-1a 64), splitmix64 and the bit
//! reader.
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

mod bitreader;
mod checksums;
mod digest;
mod distance;
mod error;
mod names;
mod sha256;
mod splitmix64;

pub use crate::bitreader::BitReader;
pub use crate::checksums::{adler32, crc32, fnv1a64};
pub use crate::digest::{Digest, Read};
pub use crate::distance::hamming;
pub use crate::error::{Error, Result};
pub use crate::names::{Algorithm, Format};
pub use crate::sha256::sha256;
pub use crate::splitmix64::SplitMix64;
