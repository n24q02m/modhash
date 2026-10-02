# `modhash-png` — PNG decoder (ISO/IEC 15948 / W3C PNG)

Scope per design spec §4.5 and the per-crate lane:

| In scope | Refused by name |
|---|---|
| All five colour types (0, 2, 3, 4, 6) | Unknown critical chunk → `Unsupported` |
| Bit depths 1/2/4/8/16 at their legal colour types | Illegal depth/type pair → `BadValue` |
| All five row filters (None/Sub/Up/Average/Paeth) | Filter byte ≥ 5 → `BadValue` |
| Adam7 interlacing, including empty passes | Non-0/1 interlace method → `BadValue` |
| `PLTE` + `tRNS` (grey, RGB, palette-alpha forms) | `tRNS` on colour types 4/6 → `BadValue` |
| Per-chunk CRC-32 on **every** chunk, ancillary included | CRC mismatch → `BadValue` |
| Ordering rules: `IHDR` first, `PLTE`/`tRNS` before `IDAT`, consecutive `IDAT`s, `IEND` last | Violation → `BadValue`, trailing bytes after `IEND` → `BadValue` |

`encode()` exists only as test scaffolding: non-interlaced, one filter
per file, stored DEFLATE blocks. It is not a compressor.

## Layout decoded, strict

```
signature 8 bytes
IHDR      13 bytes, must be first
PLTE?     required iff colour type 3; forbidden on 0/4; ≤256 entries,
          len % 3 == 0
tRNS?     ct0: 2 bytes; ct2: 6 bytes; ct3: ≤PLTE entries, must follow
          PLTE; forbidden on 4/6
ancillary* skipped by length, still CRC-checked
IDAT+     consecutive, concatenated then inflated as one zlib stream
IEND      empty, must be last; anything after is an error
```

Chunk types are four ASCII letters with the reserved (third) letter
uppercase; anything else is `BadValue` before dispatch. Critical chunks
(uppercase first letter) this decoder does not know are `Unsupported`.

## Reconstruction

Filter stride is `ceil(channels * depth / 8)` whole bytes; the first
`bpp` bytes of every row treat the "previous pixel" as zero. `Sub`,
`Up`, `Average` and `Paeth` accumulate with `wrapping_add` — PNG
residual arithmetic is defined mod 256, and a narrow subtract is a
decode-different-pixels bug, not an overflow bug.

Paeth ties resolve `a` first, then `b`, then `c` — the order is tested
by `paeth_tie_2x2.png`, whose row1 col1 has `pb == pc < pa`.

## Adam7

Passes run in spec order `(x0, y0, dx, dy)`:
`(0,0,8,8) (4,0,8,8) (0,4,4,8) (2,0,4,4) (0,2,2,4) (1,0,2,2) (0,1,1,2)`.
A pass contributes zero scanlines when its pixel width *or* height is
zero — there is no filter byte for a row that does not exist
(`adam7_2x2_gray8.png` exercises exactly this).

## Mapping to `Image`

| colour type / depth / tRNS | `Pixels` variant |
|---|---|
| 0, depth 16, no tRNS | `Gray16` |
| 0, depth ≤ 8, no tRNS | `Gray8` (sub-byte scaled by bit replication) |
| 0 + tRNS | `Rgba8`/`Rgba16` (compare raw coded-domain value) |
| 2, no tRNS | `Rgb8`/`Rgb16` |
| 2 + tRNS | `Rgba8`/`Rgba16` (compare all three raw values) |
| 3 | `Rgb8`; `Rgba8` when a `tRNS` alpha table is present |
| 4 | `Rgba8`/`Rgba16` (grey duplicated into RGB) |
| 6 | `Rgba8`/`Rgba16` |

`tRNS` comparisons run on raw coded-domain samples (a stored `u16` grey
value at depth 4 is already the 4-bit code). Palette indices past
`PLTE` are `BadValue`; alpha indices past a short `tRNS` table are
opaque, per specification.
