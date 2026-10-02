# `modhash-tier3`

Tier-3 local features: ORB, FAST-9, rBRIEF, RANSAC, D4 and DTW.

**Implemented.**

- `orb` — FAST-9 corner detection (16-pixel circle, threshold 20, strict
  `> t`), non-maximum suppression and score-ranked selection; rBRIEF:
  256-bit steered-BRIEF over a 31×31 patch, intensity-centroid
  orientation, a compile-time-generated fixed pair table confined to the
  radius-15 disk, MSB-first bitpacking.
- `ransac` — deterministic 3-point affine RANSAC (`modhash-math::solve3`,
  4 px inlier threshold, 500 seeded draws); the recovered `Affine`
  exposes `rotation()`/`scale()` for the rotation estimate between two
  matched descriptor sets.
- `d4` — the eight symmetries of the square (`transform`, `variants`)
  on `Image` via the raster crate's `rot*`/`transpose` primitives;
  square input, or `center_square` for the spec's centered crop.
- `dtw` — mean-per-step DTW with a Sakoe–Chiba band (`w = 10 %`,
  2-cell floor) in O(n·w) time / O(m) space; `band < 0` disables the
  band; unreachable endpoints report the maximum-meaningful cost `1.0`.

Fixture and test provenance notes live in `tests/tier3.rs`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-math` (#3), `modhash-raster` (#4).
- `std`, not `no_std`: orientation and rotation recovery need
  `f64::atan2`/`sin`/`cos` (same reason as `modhash-math`).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
