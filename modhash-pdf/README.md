# `modhash-pdf`

PDF text extraction across classic xref, xref streams, cmap and CID fonts.

**Status: skeleton.** The crate compiles, passes the workspace gates and has
no dependencies. Its API is specified in `docs/algorithms/` and implemented in
the phase that owns crate #18.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0), `modhash-inflate` (#1), `modhash-text` (#8).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
