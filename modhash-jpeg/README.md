# `modhash-jpeg`

JPEG decoding per ITU T.81: baseline (SOF0), extended sequential (SOF1)
and progressive (SOF2) Huffman-coded frames — spectral selection,
successive approximation and EOBRUN included — with restart markers,
4:4:4/4:2:2/4:4:0/4:2:0 and other integral sampling factors, and 1- or
3-component output as `modhash_raster::Image` (`Gray`/`Rgb`, 8-bit).

The pixel pipeline is a byte-exact transcription of libjpeg-turbo's
`islow` integer IDCT, `fancy` triangle chroma upsampling and
fixed-point YCbCr→RGB, validated against Pillow 12.3.0 fixtures byte
for byte (see `tests/fixtures/PROVENANCE.md`). Arithmetic-coded,
lossless, differential, hierarchical, 12-bit and CMYK/YCCK streams are
refused with `Error::Unsupported` naming the coding process.
`docs/algorithms/jpeg.md` pins the layout, the progressive script rules
and the deliberate divergences.

```rust
let img = modhash_jpeg::decode(&bytes)?;
match img {
    modhash_jpeg::Jpeg::Gray(g) => g.as_slice(),
    modhash_jpeg::Jpeg::Rgb(rgb) => rgb.as_slice(),
};
```

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-math` (#3), `modhash-primitives` (#0),
  `modhash-raster` (#4).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
