//! Audio fingerprint facade: spectral peaks and the peak-to-id reverse map.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table, so the whole kit
//! resolves without a single registry package.
//!
//! This crate is a skeleton. Its API is defined by `docs/algorithms/` before
//! the implementation lands in the phase that owns it.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
