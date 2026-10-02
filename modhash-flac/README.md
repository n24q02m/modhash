# `modhash-flac`

FLAC subset decoding per RFC 9639: STREAMINFO, constant and verbatim
subframes, fixed predictors of orders 0-4, and both Rice residual methods
(4-bit and 5-bit parameters, escaped partitions included). Stereo
assignments (left/side, right/side, mid/side) are decorrelated before the
PCM leaves the crate; LPC subframes surface as `Error::Unsupported`.
`docs/algorithms/flac.md` pins the exact layout, constants, and the few
deliberate deviations (local CRC-8/16, unverified MD5, limits).

```rust
let pcm = modhash_flac::decode(&bytes, &modhash_flac::Limits::default())?;
let info = modhash_flac::decode_streaminfo(&bytes)?;
```

Output mirrors `modhash-wav`'s shape: `channels`, `sample_rate`,
`bits_per_sample`, interleaved sign-extended `i32` `samples`, plus
`total_samples` provenance from STREAMINFO (0 = unknown).

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
