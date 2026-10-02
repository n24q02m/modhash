# `modhash-raster` — raw image buffers and transforms

Status: implemented. Crate `#4`, depends only on `modhash-primitives`.

## Why this crate exists

Every image codec in the kit (PNG, BMP, JPEG) decodes into the same
destination, and pHash consumes that destination. This crate is the buffer
those writers share, plus the three transforms the kit needs between a
decoded raster and a hashable raster: box-average resize, BT.601 luma, and
EXIF orientation application. Writing them once, here, keeps the rounding
conventions in one place — conventions that silently differ between
implementations are the classic source of near-miss hashes.

## Conventions that pin the numbers

### Buffer layout

- `Image<L, T>` is **tight**: `pixels.len() == width * height * CHANNELS`,
  row-major, no padding, `stride == width * CHANNELS` and therefore not
  stored. A tight buffer has exactly one representation, which is what a
  hashing kit wants; a padded variant would need a stride field, a stride
  argument on every constructor and validation of the tail bytes.
- Layout is a marker type on `Image`, not a field: `Gray` (1 channel),
  `Rgb` (3), `Rgba` (4). Sample type `T` is `u8` or `u16` (PNG 16-bit).
- Allocation is capped by `MAX_BUFFER_BYTES` (1 GiB of samples):
  `width * height * CHANNELS * size_of::<T>()` must not exceed it, and is
  computed in `u128` so dimensions near `u32::MAX` are refused, never
  wrapped. Exceeding the cap is `Error::TooLarge`, never an allocation
  attempt. A zero dimension is `Error::BadValue` — a 0×n image is a
  category error a codec should catch at its own layer.

### `box_average` resampling

One pass over the source rectangle, no separable intermediate.

- Output sample `out[y][x]` averages the source rectangle
  `rows ⌊y·H_out_ratio⌋..⌊(y+1)·H_out_ratio⌋` —
  concretely `src_y ∈ [⌊y·H/H'⌋, ⌊(y+1)·H/H'⌋)`, and likewise for `x`
  with `W/W'`. Integer floor boundaries are the spec's "accumulated
  remainder" rule made exact: box sizes differ by at most one source
  pixel and are deterministic.
- Each output sample is `round_half_up(mean)` of the source samples in
  its box, computed in `u64`: `(2·sum + n) / (2·n)`. No fractional
  weights, no float — the spec's "không hệ số phức".
- If a box is empty (only possible when **upscaling**, `out > in` on that
  axis) the output samples the single nearest source pixel at the box
  centre `⌊(2j+1)·in/(2·out)⌋`. Documented rather than refused so the
  function is total; the kit only downscales in practice (pHash → 32×32).
- Consequences: `resize(im, w, h)` with equal dimensions is the identity;
  on a constant image every output equals the constant (idempotent);
  `resize(im, 1, 1)` is the exact mean of the whole buffer.

### BT.601 luma

`Y = round_half_up(0.299·R + 0.587·G + 0.114·B)`, clamped to the sample
range (the clamp cannot fire on in-range inputs since `0.299+0.587+0.114 =
1`). Computed exactly in integers — `round_half_up((299·R + 587·G +
114·B) / 1000)` — because the decimal coefficients are exact: `299/1000 =
0.299` bit-for-bit, which a float `0.299` is not. Works on `u8` and `u16`
samples alike (u64 intermediates). Rounding is **half up**, pinned by the
spec (`round_half_up`) and by tests; `.5` rounds away from zero.

`to_gray` on `Rgb` drops no channel silently: it averages nothing, it
applies the weighted formula per pixel. On `Rgba` the alpha channel is
**ignored** (not composited): pHash needs luma of the picture, and
premultiplication is a rendering concern the hashing kit does not have.

### EXIF orientation

`apply_orientation(img, tag)` for EXIF orientation tags `1..=8`. A tag
outside that range is `Error::BadValue("exif orientation")`. Each tag is
one primitive dihedral transform; names and output geometry follow the
EXIF spec (orientation 1 = stored layout as displayed):

| tag | transform        | out dims (w,h →) |
|-----|------------------|------------------|
| 1   | identity         | w,h              |
| 2   | `flip_h`         | w,h              |
| 3   | `rot180`         | w,h              |
| 4   | `flip_v`         | w,h              |
| 5   | `transpose`      | h,w              |
| 6   | `rot90` (cw)     | h,w              |
| 7   | `transverse`     | h,w              |
| 8   | `rot270` (cw)    | h,w              |

Source-index mappings, `out(x,y) = in(cx,cy)`, `ws`/`hs` = source w/h:

| transform   | cx       | cy       |
|-------------|----------|----------|
| identity    | x        | y        |
| flip_h      | ws−1−x   | y        |
| rot180      | ws−1−x   | hs−1−y   |
| flip_v      | x        | hs−1−y   |
| transpose   | y        | x        |
| rot90 cw    | y        | hs−1−x   |
| transverse  | ws−1−y   | hs−1−x   |
| rot270 cw   | ws−1−y   | x        |

The transforms are public methods on `Image` because tier-3's D4 group is
the same eight operations. Composition is geometric (`rot90·rot90 =
rot180`, `rot90∘rot270 = identity`); the apply-twice properties are
pinned in tests.
