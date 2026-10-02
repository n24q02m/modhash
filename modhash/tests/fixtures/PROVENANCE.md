# Fixture provenance - modhash (kit facade)

Generated offline by `lab/phash_oracle.py` (Python 3.13 stdlib only: zlib/struct/math/hashlib; no image or audio library). Regenerate with `python lab/phash_oracle.py` from the repository root; the same run also rewrites `tests/phash_vectors.rs` and the `fuzz/corpus/` seeds this lane owns.

PNG fixtures are written byte-by-byte (filter 0 rows; Adam7 fixture emits per-pass scanlines in spec order). `base_444.png` is a PNG re-encode of `modhash-jpeg/tests/fixtures/base_444.raw` (Pillow's byte-exact decode of `base_444.jpg`) — the same pixels in both containers.

`l3_short.mp3` is a byte copy of `modhash-mp3/tests/fixtures/l3_short.mp3`
(ffmpeg 9.0.1 libmp3lame; see that crate's provenance) — the mp3 lane's
implemented-slot proof.

`text_page.pdf` is a byte copy of `modhash-pdf/tests/fixtures/standard_default.pdf` (WinAnsi text page; see that crate's provenance) — the pdf text lane's implemented-slot proof.

`tone.wav`/`tone.flac` carry identical PCM: `tone.wav` is minimal PCM16 RIFF; `tone.flac` is a hand-built stream of verbatim subframes (RFC 9639) — no encoder library involved.

| file | bytes | sha256:16 |
|---|---|---|
| base_444.png | 2396 | 1fea29e8e60a5099 |
| l3_short.mp3 | 4640 | 8ce6cf98f137dd7a |
| phash_adam7_37x29.png | 361 | 9760a7aba3e1eff5 |
| phash_gray16_36x28.png | 1023 | 08f72c40e384f114 |
| phash_gray8_40x32.png | 763 | c43197243100470d |
| phash_pal8_trns_52x44.png | 492 | b10baabba07f1aa8 |
| phash_rgb16_40x24.png | 5852 | 0eb2174fc167efba |
| phash_rgb8_48x40.png | 5868 | 5ffd68d91cf65683 |
| phash_rgba8_48x40.png | 7788 | 20f6e523aa5b647b |
| phash_small_13x9.png | 354 | ead4ff89c911f24a |
| tone.flac | 88363 | f954b800b5476f06 |
| tone.wav | 88244 | 041f510695ee6121 |
| tone22.wav | 44144 | 6d2dc990924d8cb1 |
| text_page.pdf | 670 | 81357e9f4320adee |
