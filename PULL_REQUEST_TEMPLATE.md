## What this changes

<!-- Which crate, and what capability. -->

## How correctness was verified

<!--
Be specific and numeric. "All tests pass" is not an answer.

- Which vectors, from which specification?
- How many cases, and what tolerance?
- If you used a property or metamorphic test, state the relation you asserted.
- For a decoder: what happens on a malformed file? Show the error, not a panic.
-->

```
cargo test --workspace
bash scripts/gate_zero_dep.sh
python scripts/gate_dag.py
```

## Checklist

- [ ] `cargo fmt --all --check` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo test --workspace` passes
- [ ] `scripts/gate_zero_dep.sh` reports `registry crates: []`
- [ ] `scripts/gate_dag.py` passes
- [ ] No new entry in any `[dependencies]`, `[dev-dependencies]` or
      `[build-dependencies]` table
- [ ] Conformance vectors live in the owning crate's `tests/`, not a shared one
- [ ] Unsupported format variants return an explicit error, never a wrong value
- [ ] `CHANGELOG.md` updated if this is user-visible
