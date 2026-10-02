# `modhash-wav`

RIFF/WAVE decoding for 8/16/24/32-bit PCM and float samples.

**Status: skeleton.** The crate compiles, passes the workspace gates and has
no dependencies. Its API is specified in `docs/algorithms/` and implemented in
the phase that owns crate #10.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
