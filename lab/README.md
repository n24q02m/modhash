# lab

Benchmarks and accuracy evaluation for the kit. Not published on crates.io.

## Layout

| path | purpose |
|---|---|
| `bench.rs` | throughput measurement with `std::time::Instant` |
| `eval.rs` | accuracy against labelled sets |
| `datasets/manifest.toml` | dataset references: URL, SHA-256, license |
| `fetch_datasets.py` | downloads and verifies the referenced datasets |

## Rules

**No vendored corpora.** `datasets/` holds a manifest, never the data itself.
Every entry names a URL, the SHA-256 the download must hash to, and the
license. `fetch_datasets.py` refuses to keep a file whose hash does not match,
so a changed upstream file fails loudly instead of silently altering every
number in a benchmark.

This is not only about repo size. A benchmark whose input cannot be re-fetched
to the same bytes is not a result, it is an anecdote.

**No `criterion`.** Measurement uses `std::time::Instant` directly. The
dependency budget of this workspace is zero, and a benchmark harness is not an
exception to a rule about reproducible fingerprints.

**Report the machine.** `bench.rs` prints CPU model, core count and whether
the build was `release`. A throughput number without those is not comparable
to anything.

## Not yet implemented

`lab` is created empty in the workspace skeleton. `bench.rs` and `eval.rs`
land with the CLI phase (P18), when there is something to measure.
