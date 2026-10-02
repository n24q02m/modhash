# `modhash-text`

Text fingerprints: lowercase, 3-word shingles and a 128-word MinHash
signature over NFC-normalised text (design spec §4).

## Pipeline

1. `canonicalize(&str) -> String` — NFC via `modhash-unicode`, then
   lowercase, then trailing whitespace stripped and exactly one `\n`
   appended. Idempotent; NFD and NFC spellings of the same text, and any
   case spelling, produce the same canonical form.
2. `signature(&str) -> Vec<u64>` — the canonical text is split into
   whitespace-separated words, windowed into consecutive `k`-word
   shingles (`k = min(3, n)`, so a one- or two-word document still
   contributes a shingle), each shingle hashed with FNV-1a 64 over the
   words joined by a single space, and the shingle set MinHashed:
   signature word `i` = `min over shingles of
   sm64_mix(fnv1a64(shingle) ^ seed_i)` where `seed_i` are drawn from
   `SplitMix64::new(SEED_STREAM)` and `sm64_mix` is the splitmix64
   output finalizer. Empty documents yield `[u64::MAX; 128]`.
3. `jaccard_estimate(&[u64], &[u64]) -> f64` — the fraction of equal
   signature words over the common prefix; standard error ≈ 0.04 at
   128 words.

`#![no_std]` + `alloc`; `#![forbid(unsafe_code)]`; deterministic on every
platform — all randomness comes from a pinned splitmix64 seed stream.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0) and `modhash-unicode` (#2).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`;
  the crate ordering is enforced by `scripts/gate_dag.py`.
