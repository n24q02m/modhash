# `modhash-unicode`

Unicode NFC normalisation driven by tables generated from the UCD.

**Status: implemented (UCD 15.1.0).** `nfc` and `nfd` implement UAX #15 —
recursive canonical decomposition, canonical ordering by combining class,
and canonical composition honouring composition exclusions — plus the
algorithmic Hangul rules of §3.12 and the NFC quick-check fast path.

The tables live in `data/ucd.bin`, a sorted binary blob embedded with
`include_bytes!` (no `build.rs`, no Rust literal arrays). It is generated
offline by `lab/gen_unicode_tables.py`; provenance, SHA-256 digests and the
spec-pinned table sizes are in `lab/ucd/PROVENANCE.md`. `data/UNICODE-LICENSE.txt`
is the Unicode License V3 the UCD files and derived tables ship under.

Conformance is `tests/NormalizationTest.txt`, the official UCD corpus
embedded into the test binary — all 19 074 data lines, plus idempotence and
the stability invariant for every unlisted scalar value.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: none outside the workspace (`modhash-primitives` only).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
