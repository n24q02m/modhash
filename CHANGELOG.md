# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.1.0] - unreleased

Initial workspace. Not published to crates.io yet: the release gate re-probes
all 23 crate names on the registry first and stops if any is taken.

### Added

- Cargo workspace with 23 crates as sibling directories, `edition = "2024"`,
  MSRV 1.85.
- Workspace policy gates, both machine-checked in CI:
  - `scripts/gate_zero_dep.sh` - fails if `cargo metadata` reports any registry
    package in the resolved graph, in any dependency table of any crate.
  - `scripts/gate_dag.py` - fails if any crate depends on a crate with an equal
    or higher number in the ordering table, which also rules out cycles.
- `scripts/publish.sh` - leaf-to-root publication order, skips crates already
  on the registry, stops at the first failure.
- Deterministic fuzz harness at `modhash/src/bin/fuzz.rs`, driven by a
  splitmix64 PRNG with four mutation modes (random, truncate, bitflip,
  repeat-insert). Reproducible from `<iters> <seed>` alone.
- CI across Linux/Windows/macOS on stable and the 1.85 MSRV, running the
  policy gates, `fmt`, `clippy -D warnings` and the workspace test suite.
- Governance: MIT license, contributing guide, security policy, code of
  conduct, issue and pull request templates, CODEOWNERS, Dependabot.

### Not yet implemented

No codec has an implementation. Each crate's API is specified in
`docs/algorithms/` and lands in the phase that owns it; see the project plan
for the phase order.

[Unreleased]: https://github.com/n24q02m/modhash/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/n24q02m/modhash/releases/tag/v0.1.0
