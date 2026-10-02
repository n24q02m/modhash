# `modhash-primitives`

Hash primitives, checksums and byte readers used across the kit.

**Status: implemented** per `docs/algorithms/primitives.md` (the one
deviation, the buffer-based `to_hex_into`, is documented there). The
crate exports `Error`/`Result`, `Digest<N>`, the `Read` trait
`Digest::from_stream` reads from, `hamming`, `Algorithm` and `Format`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: none.
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
