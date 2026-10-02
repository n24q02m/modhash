# Quickstart

Build the CLI:

```bash
cargo build --release -p modhash-cli
```

The binary is `target/release/modhash` (`modhash.exe` on Windows).

## The six commands

```bash
modhash describe <file>      # all three tiers for one input
modhash hash <file>          # tier-1 canonical hash (SHA-256)
modhash match <a> <b>        # tier-2 score + verdict
modhash dedup <dir>          # cluster a directory by tier-2
modhash bench                # tier-2 throughput per modality
modhash calibrate <csv> --profile dedup|search
```

Every command takes `--json` for machine-readable output and `--help`
for its own usage. Exit codes: `0` ok (`match`: the pair matched),
`1` pipeline refusal or no-match, `2` bad invocation or unreadable path.

## Library use

```rust
let bytes = std::fs::read("photo.png")?;
let d = modhash::describe(&bytes)?;      // format, modality, tier1, tier2, facts
let out = modhash::match_(&sa, &sb)?;    // score + advisory verdict
```

The facade is `no_std` + `alloc`; the CLI is `std`.

## What lands today

png · bmp · jpeg · wav · flac · mp3 · pdf · zip (binary semantics) ·
bare text · opaque binary. mp4 video is detected and reports
`pending: true` — its lane (`modhash-video`/`modhash-h264`) has not
landed. See [limits](limits.md) for the exact codec scope.
