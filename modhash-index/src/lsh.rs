//! Banded LSH index over MinHash signatures.
//!
//! MinHash rows with the Jaccard similarity of the underlying sets in
//! their equality probability; banding turns "similar" into "shares at
//! least one equal stripe". A signature of `bands × rows_per_band`
//! `u64` values is split into `bands` stripes of `rows_per_band`
//! values; each stripe is folded into one 64-bit bucket key, and every
//! id is filed under its `bands` keys. A query is the union of ids in
//! the matching buckets of all `bands`.
//!
//! Design spec §4.4 pins the geometry: `16 × 8` for the kit's 128-word
//! MinHash signature ([`LshIndex::new`]). Nothing else about the shape
//! is fixed — the index takes `bands` and `rows_per_band` so a caller
//! with a different signature length can re-derive a sane split.
//!
//! # What a bucket collision means
//!
//! Sharing one band key means "those `rows_per_band` MinHash values
//! were equal" — a genuine locality event, so every union member is a
//! real candidate for the full comparison. The structure never answers
//! "duplicate"; that verdict belongs to `match()` (spec §4.4: an index
//! only proposes candidates).
//!
//! # Cost and determinism
//!
//! `insert`/`candidates` are `O(bands)` map operations plus one
//! `BTreeMap` probe per band. Buckets live in `BTreeMap`s — one map per
//! band, so stripes from different bands can never meet — and queries
//! return ids sorted and de-duplicated, which keeps the whole index
//! deterministic and `no_std`-compatible.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result, SplitMix64};

/// Folds one stripe — band index and `rows_per_band` MinHash values —
/// into a 64-bit bucket key.
///
/// Every word is mixed with the workspace's shared splitmix64
/// finalizer: the accumulator XORs the word in, then one `SplitMix64`
/// output round spreads it so word position and value both matter.
/// Deterministic; equal stripes always fold to the same key.
fn stripe_key(band: usize, stripe: &[u64]) -> u64 {
    // Seed differs per band index, though bands also live in separate
    // maps; the extra spread is cheap insurance for table reuse.
    let mut acc = SplitMix64::new(band as u64).next_u64();
    for &word in stripe {
        acc = SplitMix64::new(acc ^ word).next_u64();
    }
    acc
}

/// A banded LSH index over MinHash signatures.
///
/// `bands` stripes of `rows_per_band` `u64` values each; the signature
/// length is their product. Ids are `u64` — an ordinal, a rowid or a
/// content key — and are stored opaque.
///
/// # Panics
///
/// [`LshIndex::new`] and [`LshIndex::with_shape`] panic when `bands` or
/// `rows_per_band` is zero, or when their product overflows `usize` —
/// a configuration error, never a data error.
#[derive(Debug)]
pub struct LshIndex {
    bands: usize,
    rows_per_band: usize,
    /// One bucket map per band: band key -> ids filed under it.
    buckets: Vec<BTreeMap<u64, Vec<u64>>>,
}

impl LshIndex {
    /// The kit's default geometry: 16 bands × 8 rows for the 128-word
    /// MinHash signature of `modhash-text` (design spec §4.4).
    #[must_use]
    pub fn new() -> Self {
        Self::with_shape(16, 8)
    }

    /// An index with a caller-chosen banding.
    ///
    /// `bands` stripes of `rows_per_band` values each; signatures must
    /// be exactly `bands * rows_per_band` words long.
    ///
    /// # Panics
    ///
    /// Panics on `bands == 0`, `rows_per_band == 0`, or a product that
    /// does not fit `usize`.
    #[must_use]
    pub fn with_shape(bands: usize, rows_per_band: usize) -> Self {
        assert!(bands > 0, "lsh: bands must be nonzero");
        assert!(rows_per_band > 0, "lsh: rows_per_band must be nonzero");
        bands
            .checked_mul(rows_per_band)
            .expect("lsh: bands * rows_per_band overflows usize");
        Self {
            bands,
            rows_per_band,
            buckets: alloc::vec::from_elem(BTreeMap::new(), bands),
        }
    }

    /// The signature length this index accepts:
    /// `bands * rows_per_band` words.
    #[must_use]
    pub fn signature_len(&self) -> usize {
        self.bands * self.rows_per_band
    }

    /// The number of bands.
    #[must_use]
    pub fn bands(&self) -> usize {
        self.bands
    }

    /// The number of MinHash words per band.
    #[must_use]
    pub fn rows_per_band(&self) -> usize {
        self.rows_per_band
    }

    /// Files `id` under the `bands` bucket keys of `signature`.
    ///
    /// # Errors
    ///
    /// [`Error::BadValue`] when `signature.len()` is not
    /// [`signature_len`](Self::signature_len): a truncated or padded
    /// MinHash signature would land in wrong buckets silently, so it is
    /// refused outright.
    pub fn insert(&mut self, id: u64, signature: &[u64]) -> Result<()> {
        if signature.len() != self.signature_len() {
            return Err(Error::BadValue("minhash signature length"));
        }
        for (band, stripe) in signature.chunks_exact(self.rows_per_band).enumerate() {
            let key = stripe_key(band, stripe);
            self.buckets[band].entry(key).or_default().push(id);
        }
        Ok(())
    }

    /// Every id that shares at least one band bucket with `signature`,
    /// sorted ascending and de-duplicated.
    ///
    /// The union is the candidate set for the full comparison; a
    /// bucket key collision across two different stripe values is
    /// possible in principle (64-bit fold) and merely adds one extra
    /// candidate — recall is never hurt.
    ///
    /// # Errors
    ///
    /// [`Error::BadValue`] when `signature.len()` is not
    /// [`signature_len`](Self::signature_len).
    pub fn candidates(&self, signature: &[u64]) -> Result<Vec<u64>> {
        if signature.len() != self.signature_len() {
            return Err(Error::BadValue("minhash signature length"));
        }
        let mut out = Vec::new();
        for (band, stripe) in signature.chunks_exact(self.rows_per_band).enumerate() {
            let key = stripe_key(band, stripe);
            if let Some(ids) = self.buckets[band].get(&key) {
                out.extend_from_slice(ids);
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }
}

impl Default for LshIndex {
    fn default() -> Self {
        Self::new()
    }
}
