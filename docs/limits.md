# Limits: codec scope and honest refusal

The kit decodes what it decodes. The table below is the design spec's
§4.5 codec-scope table verbatim, with a current-status column showing
what is implemented **today** — a format whose lane has not landed
answers `Unsupported` with the lane named, and a format outside the
table answers with binary semantics, never a wrong hash.

| Format | In 1.0 scope | Outside 1.0 → named error | Status today |
|---|---|---|---|
| PNG | 8/16-bit, all filters, Adam7 | — | implemented |
| BMP | 24/32-bit uncompressed + RLE8 | multi-frame images | implemented |
| JPEG | baseline + progressive (SOF0/SOF1/SOF2), 4:4:4/4:2:2/4:2:0, restart markers | arithmetic coding, lossless JPEG | implemented |
| WAV | RIFF, PCM 8/16/24/32-bit, float | compressed WAV (ADPCM) | implemented |
| FLAC | constant + verbatim, both Rice methods | — | implemented |
| MP3 | Layer I/II/III, CBR + VBR, bit reservoir | — | implemented |
| MP4/MOV | ISO-BMFF `moov`/`trak`/`stsd`/`stts`/`stsc`/`stsz`/`stco`/`co64`, `stss` | fragmented MP4 (`moof`), subtitle tracks | implemented (video lane over h264) |
| H.264 | Baseline + Main profile: CAVLC + CABAC, I/P/B slices, 8×8 transform, deblocking, 4:2:0 | High profile (4:4:4, 8×8 CABAC), HEVC, VP8/VP9, AV1, MPEG-4 Part 2 | implemented to the baseline CAVLC subset (I/P slices, no CABAC/B); above-subset elements answer `Unsupported` by name |
| ZIP | store + deflate | ZIP64, encryption, multi-disk | container implemented; hashes under binary semantics |
| PDF | classic xref + xref stream + object stream, full ToUnicode, predefined + CID fonts | encrypted PDF, digital signatures, XFA forms | implemented (text layer enters the text lane) |

## What "unsupported" means in practice

A refusal is a named error, never a silent hash:

```text
$ modhash describe clip.mp4   # e.g. HEVC, fragmented, or CABAC/high-profile h264
modality: video
format: mp4
... describe exits 1 with "video (mp4): unsupported: ..." naming the refused feature
```

`hash`, `match`, `dedup` and `calibrate` treat an unanswerable lane the
same way: exit 1, lane named on stderr, the file listed as `skipped`
in a dedup report. A format outside the crate table (GIF, EXE, unknown
bytes) is not a refusal at all: it is hashed under binary semantics,
because raw-byte tier-1 plus FastCDC chunking is the truthful answer
for bytes the kit cannot decode further.

Detection order matters: magics are checked longest-first, UTF-8 text
wins only when no container matched, and everything left is binary.
The full rule table is `docs/algorithms/kit.md` §1.

## Divergences vs the Python reference (hashkit 0.5.0)

Documented by the `lab/differential` parity run (72-file corpus):

- **PDF 8-bit encoding**: a non-symbolic `/Type1` base-14 font with no
  `/Encoding` containing a byte ≥0x80 (e.g. `\\357` = 0xEF in
  `text_page.pdf`) decodes through **WinAnsiEncoding** (0xEF → `ï`
  U+00EF), matching pypdf/Acrobat; only `/Symbol`, `/ZapfDingbats` and
  `Type3` keep their own tables (ecosystem-aligned choice,
  diff-oracle 2026-10-03).
- **MP3 PCM**: the hand-written decoder is *near bit-exact* with
  libmpg123 (via libsndfile) — aligned steady-state windows correlate
  to 1.000000 (max |d| ≈ 0.03% of full scale, sub-LSB synthesis
  rounding) on every corpus file. The one real divergence is *gapless
  trim*: libsndfile drops the LAME/Xing encoder delay (e.g. `l3_short`
  24192 vs 22050 frames) while this decoder emits every coded frame —
  same waveform, shifted. An optional gapless-trim pass is a future
  improvement, not a correctness bug. (A suspected 48 kHz polarity
  inversion was re-measured as a periodic-tone comparator artifact:
  `l3_48k` aligns at corr = 1.0.)
- **Audio canonical**: tier-1 pins 44.1 kHz mono i32 (`rate ∥ n ∥ pcm`);
  non-44.1k input is a named `Unsupported`, not a silent resample.
- **Video tier-2**: fixed 2 fps luma sampling + per-frame pHash differs
  from a cv2 all-frames pipeline — the decoded Y planes and the pHash
  math themselves are byte-identical to ffmpeg + the kit oracle.
