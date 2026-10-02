# `modhash-video`

Video fingerprint facade: mp4/mov demux → H.264 baseline decode →
2 fps frame sampling → per-frame pHash → MinHash over frame-hash
shingles (spec §4.2).

```rust
let fp = modhash_video::decode(&mp4_bytes, &modhash_video::Limits::default())?;
let m = modhash_video::video_match(&fp, &fp2);
// m.score      = fraction of temporally aligned frames within Hamming ≤ 10
// m.minhash_jaccard = MinHash-128 Jaccard over 3-frame shingles
```

Refusals are named `Err`: fragmented MP4 (`moof`), non-AVC codecs and
above-baseline H.264 (CABAC, B-frames, High profile) are `Unsupported`;
every `Limits` bound is `TooLarge`; nothing panics.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0), `modhash-raster` (#4),
  `modhash-text` (#8), `modhash-mp4` (#14), `modhash-h264` (#15).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`;
  the crate ordering is enforced by `scripts/gate_dag.py`.
