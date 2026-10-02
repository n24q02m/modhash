//! Similarity indexes over `modhash` fingerprints: a BK-tree over 64-bit
//! hashes, a banded LSH index over MinHash signatures, and threshold
//! calibration.
//!
//! Crate `#20` of the zero-dependency `modhash` workspace. Its only
//! dependency is [`modhash_primitives`] for the kit's shared error type;
//! `scripts/gate_zero_dep.sh` and `scripts/gate_dag.py` keep it that way.
//!
//! Every structure here is a **candidate index** (design spec §4.4): it
//! answers "which entries are close enough to deserve a full comparison?"
//! and nothing more. The verdict "duplicate / not duplicate" belongs to
//! the caller — the kit's `match()` — so an LSH miss or a generous radius
//! can never masquerade as a decision.
//!
//! The crate is `no_std` apart from the `alloc` containers the indexes
//! are built on, and every observable order is deterministic: ordered
//! bucket maps, distance-then-id sorted candidates, and a fixed
//! splitmix64 bucket mixer. Equal inputs produce identical output on
//! every platform and every run — the property `calibrate`'s
//! reproducibility requirement is built on.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod bktree;
mod calibrate;
mod lsh;

pub use crate::bktree::{BkTree, Candidate};
pub use crate::calibrate::{Calibration, Profile, calibrate};
pub use crate::lsh::LshIndex;
