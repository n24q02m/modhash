# `modhash-fastcdc`

FastCDC content-defined chunking with a 16-level gear mask table.

Implements the FastCDC algorithm (Wen Xia et al., USENIX ATC 2016): Gear
rolling fingerprint, zero-padded `fp & mask == 0` hash judgment,
sub-minimum cut-point skipping, and normalized chunking across the
16-level mask table. Also exports `Buzhash64`, a rotate-XOR rolling hash
over a fixed 64-byte window, for chunk fingerprinting. Specification and
verbatim constants: `docs/algorithms/fastcdc.md`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
