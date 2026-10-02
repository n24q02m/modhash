# `modhash-zip`

ZIP container reading: stored and deflated entries, plus a minimal XML reader.

**Status: skeleton.** The crate compiles, passes the workspace gates and has
no dependencies. Its API is specified in `docs/algorithms/` and implemented in
the phase that owns crate #17.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-inflate` (#1).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
