# `modhash-audio`

Audio fingerprint facade: spectral peaks and the peak-to-id reverse map.

`signature` extracts Shazam-style landmarks — `(t, f)` pairs where `t`
is a 2048-sample frame index and `f` a max-pooled frequency slot — from
interleaved `i32` PCM at 44 100 Hz (the exact output shape of
`modhash-wav` and `modhash-flac`; `signature_of_wav` /
`signature_of_flac` wrap those decoders and refuse other rates).
`build_index` inverts the corpus into `f → (t, id)`;
`match_signature` votes on a `Δt` histogram and returns the modal
offset per id within ±1 frame (≈46 ms). `Signature::fingerprint`
produces the §4.2 64-bit first-peak presence mask.

The full algorithm — constants, quantization, tie-breaking, hostile
input — lives in `docs/algorithms/audio-peaks.md`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-math` (#3), `modhash-wav` (#10), `modhash-flac` (#11).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
