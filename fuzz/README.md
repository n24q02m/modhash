# fuzz

Corpus and crash artifacts for the fuzz harness.

The harness itself lives at `modhash/src/bin/fuzz.rs` — one binary for the
whole workspace, because the `modhash` crate sits at the top of the dependency
DAG and can reach every other crate:

```bash
cargo run --release -p modhash --bin fuzz -- <target> <iters> <seed>
```

## Layout

```
fuzz/
  corpus/<target>/     seed inputs, one file per case
  crashes/<target>/    inputs that produced a panic; one file per crash
```

Both directories are created empty with the skeleton. Each codec phase
registers its target name in `TARGET_NAMES` and adds a `dispatch` arm, then
seeds `corpus/<target>/`.

## Reproducing a crash

Everything is driven by a splitmix64 PRNG seeded from the command line, so a
failing iteration is reproducible from `<target> <iters> <seed>` alone, on any
machine, without the corpus. The command line belongs in the bug report.

## Mutation modes

`random`, `truncate`, `bitflip`, `repeat-insert` — the four modes cycle across
iterations so every codec is exercised in all of them.

## Do not commit large binaries

A crash file is worth committing because it is small and reproducible. A
200 MB seed corpus is not: it belongs in storage, referenced by checksum, the
same rule `lab/datasets` follows.
