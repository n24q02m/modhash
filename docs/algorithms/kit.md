# The kit facade: detection, tier 1, tier 2, match, describe

Crate `modhash` (#21, top of the DAG) wires the modality crates into one
entry surface. This file is the written-down version of every decision a
second implementer would otherwise have to guess: the exact bytes each
tier-1 digest is computed over, the exact bits a pHash bit is set by, and
which inputs are refused instead of answered.

Two kinds of answer exist and are never conflated:

- **`Err`** — the input could not be given the requested semantics
  (undecodable, wrong rate, a modality slot whose crate has not landed).
  Every error carries the modality name.
- **A signature/score** — a real, reproducible value.

`match` and `describe` never panic and never guess: garbage in is a named
`Err` out.

## 1. Format detection

`detect(bytes)` sniffs magic in the order below — first match wins. Longer
magics are checked before their prefixes, so no row shadows another.

| magic (offset 0 unless noted) | `Format` | `Modality` | state |
|---|---|---|---|
| `89 50 4E 47 0D 0A 1A 0A` | `Png` | `Image` | implemented |
| `FF D8 FF` | `Jpeg` | `Image` | implemented |
| `42 4D` (`"BM"`) | `Bmp` | `Image` | implemented |
| `47 49 46 38` (`"GIF8"`) | `Gif` | `Binary` | no GIF crate exists in the workspace; falls back to binary semantics |
| `52 49 46 46` + `57 41 56 45` at offset 8 | `Wav` | `Audio` | implemented |
| `66 4C 61 43` (`"fLaC"`) | `Flac` | `Audio` | implemented |
| `49 44 33` (`"ID3"`), or `FF` + second byte `& E0 == E0` with MPEG version bits `& 18 != 08` and layer bits `& 06 != 00` | `Mp3` | `Audio` | **`Unsupported`** — `modhash-mp3` has not landed |
| 4-byte size + `66 74 79 70` (`"ftyp"` at offset 4) | `Mp4` | `Video` | **`Unsupported`** — `modhash-video`/`modhash-h264` have not landed |
| `25 50 44 46 2D` (`"%PDF-"`) | `Pdf` | `Text` | **`Unsupported`** — `modhash-pdf` has not landed |
| `50 4B` (`"PK"`) | `Zip` | `Binary` | raw-byte semantics |
| valid UTF-8, none of the above | `Unknown` | `Text` | implemented |
| anything else | `Unknown` | `Binary` | implemented |

Two rules decide the unsupported/binary split, and they are the *only*
rules:

- A format whose **crate exists in the DAG table but has not landed**
  (mp3, mp4→video, pdf→text) answers `Error::Unsupported { modality, .. }`
  naming the modality its crate will serve. The slot is named and real;
  the answer is "not yet", not "binary blob".
- A format **outside the workspace's crate table entirely** (GIF, ZIP,
  unknown binaries) is `Binary`: raw-byte hashing and FastCDC chunking
  are the honest answer, and they always apply.

`Format::Unknown` is the wire name for content with no container: UTF-8
text and opaque binary both report `Format::Unknown`; `modality`
distinguishes them.

## 2. Tier 1 — `content_hash` (SHA-256 over normalized content)

`content_hash` returns `Digest<32>` = SHA-256 of the **normalized
content**, per modality. The digest input is exactly the bytes listed —
no container bytes, no metadata, no trailing length padding beyond what
is listed.

### image

The interleaved `R G B` `u8` triples of the decoded image in canonical
raster order (row-major, top-left origin — the order every decoder in
the kit emits). Normalization into that form:

- `Gray` `v` → the triple `(v, v, v)`;
- `Rgba` `(r,g,b,a)` → `(r, g, b)` — **alpha is dropped**, matching the
  luma convention (alpha is not part of the image's colour content);
- `u16` samples → `u8` by `v >> 8` (discard the low byte — PNG's
  sanctioned 16→8 reduction).

Consequence, pinned by test: the same pixels carried by PNG, JPEG or BMP
produce the same tier-1 digest whenever the decoders agree byte-for-byte
(BMP always does; JPEG does for the lossless-inside-tolerance cases the
modality crates' own fixtures demonstrate, and any residual difference
is a *pixel* difference, not a facade one).

### audio

SHA-256 input:

```
u32_le  sample_rate          (as decoded; not constrained to 44100)
u64_le  frame_count          (mono frames after downmix)
i32_le × frame_count         (mono PCM, the kit's canonical i32 domain)
```

Downmix to mono before hashing, same convention as
`modhash_audio::signature`: `m[i] = (Σ_c samples[i·ch + c]) / ch` in
`i64`, truncated toward zero. `samples` are the decoder's canonical
`i32` (sign-extended, *not* rescaled — a 16-bit −2 is −2, not
−2·2¹⁶).

Consequence, pinned by test: WAV and FLAC encodings of the same PCM
produce the same digest, because both decoders emit the same `i32`
stream; different sample rates or different lengths produce different
digests through the header words.

### text

SHA-256 over `modhash_text::canonicalize(text)` encoded as UTF-8 — NFC,
lowercased, trailing whitespace stripped and exactly one `'\n'`
appended. NFC and NFD spellings of the same text hash identically; so
do trailing-newline variants and upper/lowercase pairs.

`content_hash` on text requires `&str` input. The byte-level entry
`signature(bytes)` / `describe(bytes)` applies `str::from_utf8`; a
non-UTF-8 input is `Binary`, never a silent lossy decode.

### binary

SHA-256 over the raw bytes, unchanged.

## 3. Tier 2 — `image_phash` (64-bit perceptual hash)

Per §4.2, exactly these steps, in this order, over an image already
decoded to `Image<_, u8>` (any layout; `u16` inputs first reduce to `u8`
by `v >> 8`):

1. **Luma**: `modhash_raster::luma_bt601` per pixel —
   `round_half_up((299·r + 587·g + 114·b) / 1000)`, clamp `[0,255]`
   (alpha ignored; `Gray` pixels are already luma). Result: one
   `Gray` `u8` image at the source size.
2. **Resize**: `modhash_raster::box_average` to exactly **32×32**
   (integer box average, `round_half_up` inside).
3. **DCT**: `modhash_math::dct2_2d` (orthonormal DCT-II, rows then
   columns) on the 32×32 `f64` block.
4. **Crop**: keep the low-frequency **8×8** block (coefficients
   `[y][x]`, `x,y ∈ 0..8`).
5. **Threshold**: `t = ` **median of the 63 coefficients excluding DC**
   `[0][0]` — `modhash_math::median`, the workspace's lower-middle
   convention (`v[(n−1)/2]` after sorting; for 63 elements, index 31).
6. **Bits**: 64 bits, row-major over the kept 8×8 — coefficient `(y,x)`
   goes to bit `63 − (y·8 + x)`, i.e. **MSB is `[0][0]`** (the DC bit),
   LSB is `[7][7]`. Bit = 1 iff `coeff > t`. DC is *compared* against
   the same median but never participates in it.

Distance between two pHashes is `modhash_primitives::hamming`. The
duplicate-acceptance bound is `hamming ≤ 10` (`match` reports it as
`matched`); the pinned JPEG-vs-PNG property is `hamming ≤ 4` for
byte-identical pixel content.

## 4. Tier 2 — text

`signature(text)` is `modhash_text::signature(canonical-form input)`:
128-word MinHash over FNV-1a hashes of consecutive 3-word shingles of
the canonical text (documents under 3 words use `k = n`). Distance is
`modhash_text::jaccard_estimate` — fraction of equal words.

The NFC/NFD guarantee of tier 1 holds at tier 2: the signature is
computed *after* canonicalization, so equivalent spellings produce
identical signatures, not merely similar ones.

## 5. Tier 2 — binary

`FastCdc::new(bytes, min=2048, avg=8192, max=32768)`
(default normalization `Level2`) over the raw bytes; each chunk's
**SHA-256** digest forms the signature as a `BTreeSet<[u8;32]>` —
**unique digests only** (the set deduplicates repeated chunks;
cardinality semantics, not multiset).

Similarity is exact Jaccard over the two sets: `|A ∩ B| / |A ∪ B|`,
with `∅ vs ∅ = 1.0` (two empty files are identical).

Property, pinned by test: same bytes under different names → identical
chunk set and identical tier-1; one inserted prefix byte shifts chunk
boundaries only locally → high Jaccard, different tier-1.

## 6. Tier 2 — audio

`signature(bytes)` → `modhash_audio::signature_of_wav` /
`signature_of_flac` (44 100 Hz gate included, per `audio-peaks.md`).
`match` runs `build_index` on the right operand's signature, queries
with the left, and reports the top `Match`: `delta_t` (modal frame
offset, positive = query starts later), `votes` (histogram peak mass).

`matched` is advisory: `votes ≥ 8` is the kit's floor — one coincidental
shared slot is not a match. Callers needing recall tune on the raw
`votes`, which is always reported.

## 7. `describe`, `match`, errors

`describe(bytes)` = detect → decode → both tiers, returned as one
`Description`: `format`, `modality`, `tier1`, and a `Tier2Report`
carrying the modality-specific fields (pHash + dimensions for image;
peak/frame/rate counts for audio; signature head + word count for text;
chunk count + byte length for binary). Any refusal at any stage is the
returned `Err` — a description is complete or absent.

`match(a, b)` compares two `Signature`s. **Modality mismatch is
`Error::BadValue`**, not a zero score — a pHash and a MinHash signature
have no defined distance, and returning `0.0` would let a caller file
it as "definitely different".

Every modality error maps into `modhash::Error` with the modality name
attached (`Error::Decode { modality, source }` wrapping the codec's
`modhash_primitives::Error`; `Error::Audio` for the `no_std` audio
crate's own error type, whose `Display` prefixes `audio:`). No
`unwrap`/`expect` appears on a path reachable from public input.

## 8. Fuzz

`modhash/src/bin/fuzz.rs` owns the workspace harness. The facade phase
registers corpus-backed targets `png`, `jpeg`, `bmp`, `audio` (the
audio arm feeds every iteration through `wav::decode`, `flac::decode`,
`signature_of_wav` and `signature_of_flac`), alongside the pre-existing
`mp4`/`flac`/`coremode` arms. Seeds live in `fuzz/corpus/<target>/`
with provenance in `fuzz/corpus/PROVENANCE.md`.
