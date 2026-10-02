# `modhash`

The kit facade: canonical hash, tier 1, tier 2, match and describe.

**Status: skeleton.** The crate compiles, passes the workspace gates and has
no dependencies. Its API is specified in `docs/algorithms/` and implemented in
the phase that owns crate #21.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0), `modhash-inflate` (#1), `modhash-unicode` (#2), `modhash-math` (#3), `modhash-raster` (#4), `modhash-png` (#5), `modhash-bmp` (#6), `modhash-jpeg` (#7), `modhash-text` (#8), `modhash-fastcdc` (#9), `modhash-wav` (#10), `modhash-flac` (#11), `modhash-mp3` (#12), `modhash-audio` (#13), `modhash-mp4` (#14), `modhash-h264` (#15), `modhash-video` (#16), `modhash-zip` (#17), `modhash-pdf` (#18), `modhash-tier3` (#19), `modhash-index` (#20).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
