# `modhash-png`

PNG decoding for 1/2/4/8/16-bit images of every colour type: all five
row filters, Adam7 interlacing, palette lookup and `tRNS` transparency,
decoding into `modhash-raster`'s `Image` buffers.

**Status: implemented.** The decoder walks chunks with per-chunk CRC-32
([`modhash-primitives`](../modhash-primitives)), inflates `IDAT` through
[`modhash-inflate`](../modhash-inflate), enforces chunk ordering, and
maps every legal bit-depth/colour-type combination to a concrete pixel
buffer (`Gray8`/`Gray16`/`Rgb8`/`Rgb16`/`Rgba8`/`Rgba16`). Sub-byte
greyscale scales to 8-bit by bit replication; grey+alpha and any `tRNS`
widen to `Rgba`. Every malformed input is a named `Error`, never a panic.

`encode()` is a **minimal, non-interlaced, single-filter encoder kept
public only as test scaffolding** — it writes stored DEFLATE blocks and
exists so `decode(encode(x)) == x` round-trips and fixture
cross-checking (`filtered == unfiltered`) are possible without trusting
the decoder to write its own reference.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not
  optional.
- Dependencies: `modhash-primitives` (#0), `modhash-inflate` (#1),
  `modhash-raster` (#4).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`;
  the crate ordering is enforced by `scripts/gate_dag.py`.

## Tests

`cargo test -p modhash-png` runs the conformance suite against the byte
fixtures in `tests/fixtures/` (see `PROVENANCE.md` there), the 200-image
seed-fixed round-trip, strict-prefix corruption loops and a 10 000
single-byte mutation loop driven by `SplitMix64`.
