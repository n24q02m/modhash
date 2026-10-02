# Concepts: the three tiers

`modhash` answers three different questions about a file, and keeps them
strictly separate:

## Tier 1 — canonical hash

`modhash hash` / `modhash::content_hash`. SHA-256 over the *normalized
content*, never the container:

- image → decoded `R G B` u8 triples (alpha dropped, u16 halved)
- audio → `rate ∥ frame_count ∥ mono i32 PCM`
- text → UTF-8 of the canonical form (NFC, lowercase, one `\n`)
- binary → the raw bytes

Two encodings of the same pixels (PNG vs BMP) or the same PCM (WAV vs
FLAC) hash identically. Different bytes in, same content out ⇒ same
digest. This is the dedup/equality primitive.

## Tier 2 — modality signature

The perceptual or content-defined fingerprint `modhash match` compares:

| modality | signature | distance | advisory match bound |
|---|---|---|---|
| image | 64-bit pHash (32×32 box → DCT-II → low 8×8 → median bit) | Hamming | ≤ 10 |
| audio | spectral-peak landmarks (Shazam-style, 44.1 kHz) | Δt histogram votes | votes ≥ 8 |
| text | 128-word MinHash over word-3 shingles | Jaccard estimate | ≥ 0.8 |
| binary | FastCDC chunk SHA-256 set (2K/8K/32K) | Jaccard | ≥ 0.5 |

`match()` refuses cross-modality pairs — no distance between a pHash and
a MinHash vector is defined.

## Tier 3 — local features

Classical (non-ML) descriptors for geometric verification: ORB (FAST-9 +
steered rBRIEF-256), RANSAC affine estimation, D4 symmetries, banded DTW
(Sakoe–Chiba 10%). `describe` reports the image tier-3 feature count;
matching against tier-3 features is a library call (`modhash-tier3`).

## Detection and honesty

`detect` sniffs container magic first, then UTF-8 ⇒ text, else binary.
A format whose crate has not landed answers `Unsupported` with the lane
named — never a hash of container noise. A format outside the codec
table entirely (GIF, arbitrary binaries) is honestly binary.
