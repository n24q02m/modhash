# lab

Benchmarks and accuracy evaluation for the kit. Not published on crates.io.

## Layout

| path | purpose |
|---|---|
| `bench.rs` | throughput measurement with `std::time::Instant` |
| `eval.rs` | accuracy against labelled sets |
| `datasets/manifest.toml` | dataset references: URL, SHA-256, license |
| `fetch_datasets.py` | downloads and verifies the referenced datasets |
| `smoke_all_formats.sh` | end-to-end CLI run over every landed §4.5 format |
| `smoke-fixtures/` | small committed inputs for the smoke script |

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

## How to run

```bash
# library-level throughput (in-process, release profile)
cargo run --release -p modhash-cli -- bench

# CLI end-to-end latency (spawn + IO + all tiers)
cargo build --release -p modhash-cli
rustc -O lab/bench.rs -o target/release/lab-bench
./target/release/lab-bench target/release/modhash

# accuracy on a labelled pairs file
rustc -O lab/eval.rs -o target/release/lab-eval
./target/release/lab-eval pairs.csv target/release/modhash

# every landed format, end to end
bash lab/smoke_all_formats.sh
```

`lab/smoke-fixtures/` holds the small committed inputs the smoke script
exercises; they are fixtures, not corpora — see its `PROVENANCE.md`.
