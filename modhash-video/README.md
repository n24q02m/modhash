# `modhash-video`

Video fingerprint facade: frames to perceptual hashes to sequence matching.

**Status: skeleton.** The crate compiles, passes the workspace gates and has
no dependencies. Its API is specified in `docs/algorithms/` and implemented in
the phase that owns crate #16.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0), `modhash-raster` (#4), `modhash-text` (#8), `modhash-mp4` (#14), `modhash-h264` (#15).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
