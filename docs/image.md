# Image modality

**Formats:** png, bmp, jpeg. `gif` is detected but has no image lane —
it answers as binary (raw-byte semantics), by rule.

**Tier 1** decodes to canonical `RGB u8` (gray replicated, alpha
dropped, u16 halved by `>> 8`) and hashes the pixel stream. Identical
pixels in different containers share the digest — pinned by test on
`base_444` png/jpeg.

**Tier 2** is the 64-bit pHash: luma (BT.601) → 32×32 box average →
DCT-II → low 8×8 → one bit per coefficient above the median of the other
63. Distance is Hamming; `match` calls a pair matched at `hamming ≤ 10`.

**Tier 3** is ORB: FAST-9 corners + steered rBRIEF-256 descriptors,
with RANSAC affine verification and the eight D4 symmetries.
`describe` reports the FAST-9 keypoint count.

```bash
modhash describe photo.png
modhash match a.jpg b.png        # same pixels, different containers
modhash dedup ./photos           # cluster by pHash
```
