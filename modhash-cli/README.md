# `modhash-cli`

The `modhash` command line interface.

```text
modhash describe <file>     all three tiers: canonical hash, modality
                            signature, local features
modhash hash <file>         tier-1 canonical hash (SHA-256)
modhash match <a> <b>       tier-2 score + verdict at the advisory bound
modhash dedup <dir>         cluster a directory by tier-2 signature
modhash bench               tier-2 throughput per modality (wall-clock)
modhash calibrate <csv> --profile dedup|search
                            fit a match threshold on labelled pairs
```

`--json` selects machine-readable output. `modhash <command> --help`
documents that command's options and exit codes.

## Exit codes

| code | meaning |
|---|---|
| 0 | answer produced (`match`: the pair matched) |
| 1 | pipeline refusal — unsupported slot, corrupt container — or `match` answered no-match |
| 2 | bad invocation or unreadable path |

A detected format whose lane has not landed (mp4 video today) is a
report, not a crash: `describe` prints `pending: true` and exits 0;
`hash`/`match`/`dedup` exit 1 with the lane named.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash` (#21) only.
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
