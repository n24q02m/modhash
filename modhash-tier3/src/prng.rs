//! A private copy of the splitmix64 recurrence.
//!
//! The workspace ordering gate (`scripts/gate_dag.py`) declares this crate's
//! edges as `modhash-math` + `modhash-raster` only, so the shared
//! `modhash_primitives::SplitMix64` is not reachable from here. The pinned
//! constants — the rBRIEF test-pair table and the RANSAC sampler — need a
//! deterministic 64-bit PRNG, so the well-known splitmix64 finalizer is
//! transcribed once at this call site. `const fn` so the BRIEF pair table is
//! generated at compile time with zero runtime cost.

/// Advance `state` by the golden-ratio increment and emit the z-mixed word.
pub(crate) const fn splitmix64_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
