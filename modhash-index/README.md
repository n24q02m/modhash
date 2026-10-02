# `modhash-index`

Similarity indexes: BK-tree, LSH and threshold calibration.

**Implemented.** `BkTree` indexes `(u64 hash, u64 id)` pairs under Hamming
distance with an iterative arena walk — sorted results, no recursion, so
attacker-sized trees cannot overflow the call stack. `LshIndex` buckets
MinHash signatures into per-band ordered maps keyed by a splitmix64 stripe
fold; the default geometry is the spec's 16 bands × 8 rows over the
128-word `modhash-text` signature, and `with_shape` accepts other splits.
`calibrate` measures a matching threshold on labelled
`(distance, is_match)` pairs under two profiles — `Dedup` (F0.5,
precision-weighted) and `Search` (F2, recall-weighted) — with a
deterministic tie-break, and may answer "match nothing". Per spec §4.4
the indexes only propose candidates; the verdict belongs to `match()`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
