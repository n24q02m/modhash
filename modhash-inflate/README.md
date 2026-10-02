# `modhash-inflate`

DEFLATE and zlib stream decoding (RFC 1951).

**Status: skeleton.** The crate compiles, passes the workspace gates and has
no dependencies. Its API is specified in `docs/algorithms/` and implemented in
the phase that owns crate #1.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: none.
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
