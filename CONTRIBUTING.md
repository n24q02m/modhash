# Contributing to modhash

Thanks for looking. This project is small and opinionated; the opinions are
the point.

## The one rule that is not negotiable

**Zero external dependencies.** No crate in this workspace may name a package
in `[dependencies]`, `[dev-dependencies]` or `[build-dependencies]` — including
a subprocess call to a binary that happens to be installed on the machine.
`scripts/gate_zero_dep.sh` fails CI if one appears.

If you need a hash, a codec, a matrix routine, an XML parser or a fuzzing
primitive, implement it in the crate that owns that capability, and back it
with a conformance suite.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/gate_zero_dep.sh
python scripts/gate_dag.py
```

All five must be clean locally. CI runs the same commands on Linux, Windows
and macOS, on stable and on the 1.85 MSRV.

## Dependency direction

Crates are numbered 0 to 22. A crate may only depend on a crate with a
**lower** number. That is what makes the DAG acyclic by construction, and it
means the workspace can always be built in one linear pass.

If your change needs crate 12 to call into crate 19, the answer is not to add
the edge. Either the capability belongs lower in the tree, or the higher crate
should call into the lower one instead.

## Conformance, not self-written tests

Where a specification publishes test vectors, use them. The Unicode NFC
crate runs the UCD `NormalizationTest.txt`; the PNG crate is checked against
the specification's own images; `modhash-primitives` is checked against
FIPS 180-2. A test you wrote yourself proves only that your code agrees with
your own understanding.

For anything without published vectors, a metamorphic or property test is the
next best thing: assert a relation that must hold regardless of the input
(`decode(encode(x)) == x`, `resize_to_1x1(x) == mean(x)`).

## Crates and public API

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are set in every crate.
  If you believe you need `unsafe`, the design is wrong; say so in an issue
  rather than opening the lint locally.
- Document the *why* on public items. What the function does is usually
  obvious from its name.

## Commit and PR conventions

- One crate per pull request where practical. The conformance suite for that
  crate belongs in the same pull request.
- Describe how you verified correctness, in numbers: which vectors, how many
  cases, what tolerance.
- If a format variant is deliberately unsupported (arithmetic-coded JPEG,
  fragmented MP4, encrypted ZIP), the decoder must return an explicit error.
  Silently mis-decoding is the one outcome this project will not ship.

## License

By contributing you agree your work is licensed under the MIT License, the
same as the rest of the project.
