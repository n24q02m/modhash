# modhash

A zero-dependency hashing kit for Rust: content hashes for files, perceptual
hashes for images, text, audio and video, and an index layer for similarity
search. Every crate resolves with an empty `[dependencies]` table.

> **Status: skeleton.** The workspace, its policy gates and CI are in place. No
> codec is implemented yet; each crate's API is specified in `docs/algorithms/`
> and lands in the phase that owns it.

## Why zero-dependency

Two reasons, both practical:

- A fingerprint must be reproducible years from now. A dependency tree is a
  supply chain that can change, be yanked, or relicense under you. With zero
  dependencies, the exact bytes that produced a hash are still in this repo.
- Auditability. Every line that touches your input is visible in one place.

The rule is machine-checked. `scripts/gate_zero_dep.sh` fails if
`cargo metadata` reports a single registry package in the resolved graph.

## Layout

One repository, one Cargo workspace, 23 crates as sibling directories. Cargo
treats a directory as a crate when it has its own `Cargo.toml`; crates are not
nested inside a shared `src/`.

| # | crate | what it owns |
|---|---|---|
| 0 | `modhash-primitives` | sha256, fnv1a64, crc32, adler32, hex, splitmix64, bitreader |
| 1 | `modhash-inflate` | DEFLATE / zlib / raw (RFC 1951) |
| 2 | `modhash-unicode` | NFC, tables generated from the UCD |
| 3 | `modhash-math` | FFT, DCT-II, median, solve3, linalg |
| 4 | `modhash-raster` | image buffers, box-average, BT.601, EXIF orientation |
| 5 | `modhash-png` | PNG 8/16-bit, Adam7, filters |
| 6 | `modhash-bmp` | BMP 24/32-bit, RLE8 |
| 7 | `modhash-jpeg` | JPEG baseline + progressive |
| 8 | `modhash-text` | lowercase, 3-shingles, MinHash-128 |
| 9 | `modhash-fastcdc` | FastCDC, buzhash, 16-level mask table |
| 10 | `modhash-wav` | RIFF/WAVE, PCM 8/16/24/32 + float |
| 11 | `modhash-flac` | FLAC subset, both Rice methods |
| 12 | `modhash-mp3` | MP3 layers I/II/III, bit reservoir |
| 13 | `modhash-audio` | spectral peaks, peak-to-id reverse map |
| 14 | `modhash-mp4` | ISO-BMFF demux, avcC, sample tables |
| 15 | `modhash-h264` | NAL, SPS, CAVLC/CABAC, transforms, deblocking |
| 16 | `modhash-video` | frames to perceptual hash to sequence match |
| 17 | `modhash-zip` | ZIP store + deflate, minimal XML |
| 18 | `modhash-pdf` | xref, object streams, cmap, CID fonts |
| 19 | `modhash-tier3` | ORB, FAST-9, rBRIEF, RANSAC, D4, DTW |
| 20 | `modhash-index` | BK-tree, LSH, threshold calibration |
| 21 | `modhash` | the kit: canonical, tier 1, tier 2, `match()`, `describe()` |
| 22 | `modhash-cli` | the `modhash` command |

## Workspace rules

These are enforced, not merely documented:

- **A crate may only depend on a crate with a lower number.** This single rule
  also rules out dependency cycles. `scripts/gate_dag.py` fails the build
  otherwise.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` in every crate.
- Each crate owns its conformance suite in its own `tests/`.

## Commands

```bash
cargo build --workspace --offline
cargo test  --workspace

bash scripts/gate_zero_dep.sh     # no registry package anywhere in the graph
python scripts/gate_dag.py        # every edge points to a lower crate number

cargo run --release -p modhash --bin fuzz -- coremode 10000 13
```

`scripts/publish.sh` publishes leaf-to-root and stops at the first failure, so
a dependency chain is never left half-published.

## License

MIT. See [LICENSE](LICENSE).

Generated tables in `modhash-unicode/data/` come from the Unicode Character
Database and are covered by the Unicode License; the notice ships alongside
them as `data/UNICODE-LICENSE.txt`.
