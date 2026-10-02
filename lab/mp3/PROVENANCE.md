# modhash-mp3 table provenance

All generated tables in `modhash-mp3/src/{tables,hufftab}.rs` are produced by
`lab/mp3/gen_mp3_tables.py` and cross-checked against at least two independent
references before landing — the generator aborts (non-zero exit) when sources
disagree.

## Normative source

- `lab/mp3/ref/iso11172-3/ANNEX_AB.txt` — OCR of ISO/IEC 11172-3 Annex A/B
  (Huffman Table 3-B.7, window/transform formulas §2.4.3.4, polyphase §2.4.3.2,
  allocation Tables B.2/B.1, sfb tables B.8).
  sha256 `b05172d2a28c73f40ac5ef3a8a2615683404a486b3a78fa36c3136d0fc1523eb`
- `lab/mp3/ref/iso11172-3/MPGAUDIO.txt` — OCR of the spec body (§2.4.3
  decode process, side info, scalefactor layout).
  sha256 `2b72d58573a270ff94711c4f2a784e5d38f3d9d7a6837cfe5d4a6fc5201b8d1d`
- `lab/mp3/annex_b.txt` — earlier OCR pass of the same annex.
  sha256 `a51ab96be404e4ee4404a476d0795aee61a1df6f8c65961434533e6e34917173`

## Independent implementations cross-checked

Every table row was verified against ≥2 of these; disagreements were resolved
against the spec text.

| file | role | sha256 |
|---|---|---|
| `ref/minimp3.h` | Lieff minimp3 — nested Huffman tables, sfbtab, scf math | `57e437c5c1f0e8b243885d3929c8973b5e6c778451e0100ab4251d19915cb3ad` |
| `ref/mad_huffman.c` | libmad Huffman tables | `ab7bbad837fda91d8382b936d86c53cb8f415404f20da568294fe099730a5c22` |
| `ref/mad_layer3.c` | libmad layer III flow (reorder, imdct, alias) | `e59e003561865caa34601946057cac2576b444513888959fc6a0fe6ea2a73ba8` |
| `ref/mad_layer12.c` | libmad layers I/II (dequant C/D, alloc) | `16e77a72196cfdeacc5ea780b169e94151bcf87fc03e60f3660b73df6d1e12d8` |
| `ref/mad_synth.c` | libmad polyphase synth | `c7a49ee84744f3d6a7ef2fa5be69bfc3d65056b9748fc58978843df0deafd5f5` |
| `ref/mpg123_frame.c` | mpg123 side-info/scalefactor conventions | `bd526ac84434579dccfc8b293e7aabe9ad0d3d824301237bb090cda4bc0fff68` |
| `ref/mpg123_huffman.h` | mpg123 Huffman tables | `d8ad16c4486e99c83f7c2c0fee94c76a5c1c83136c8e324e1d11087334e51085` |
| `ref/mpg123_dct64.c` | mpg123 polyphase DCT64 | `d56adc06ce905171da0ec60bd4b65c92e2f0084b53fd59416c24a9520a182b6b` |
| `ref/ff_mpegaudiodata.c` | ffmpeg static tables | `4b36b54983ef898b5ef4ae4abd593350c96b49b5dec2910b8d566c90a67f127d` |
| `ref/ff_mpegaudiodec.c` | ffmpeg decode (alloc, scf, dequant) | `4e4a4ac7e6b44ce175af50f3536dc6462583a012afcc3f5c8fefec79b7bd78ae` |
| `ref/dr_mp3.h` | dr_mp3 (minimp3 fork, API reference) | `997b7ee18de6e6b81e2a83f1ea9fc62aef25c62b28d48db95635f49e65de0a2f` |
| `ref/jlayer_SynthesisFilter.java` | jlayer polyphase | `9636b658bd288b643271a83edb5f484182992614cc908e4cc81cde8ebf65568b` |
| `ref/mpegaudiodata.h` | ffmpeg header tables | `80159a5e7a741dcc451985e1e373862b71d09b7763fdd5863518fc631ca46b1e` |

Upstream sources of record (version the files were taken from):

- minimp3: <https://github.com/lieff/minimp3> — `minimp3.h` public-domain implementation.
- libmad: <https://www.underbit.com/products/mad/> / distribution tarball `libmad-0.15.1b` — `mad_*.c/h`, `qc_table.dat`, `mad_sf_table.dat`, `mad_rq_table.dat`.
- mpg123: <https://mpg123.de> / `mpg123-1.x` source — `mpg123_*.c/h`.
- ffmpeg: <https://ffmpeg.org/download.html> — `ff_mpegaudio*` sources (tables + decoder, v9.0.1 tree).
- dr_mp3: <https://github.com/mackron/dr_libs> — `dr_mp3.h`.
- JLayer: <https://www.javazoom.net/javalayer/javalayer.html> — `SynthesisFilter.java`.
- ISO/IEC 11172-3: OCR capture of the normative text kept under `ref/iso11172-3/`.

`gen_mp3_tables.py` parses the spec OCR and walks minimp3's nested tables;
disagreement aborts generation (the script exits non-zero).

## Decoded-reference fixtures

`modhash-mp3/tests/fixtures/*.mp3` / `*.mp2` / `*.mp1` were encoded offline with
ffmpeg 9.0.1 (`-c:a libmp3lame` for L3, `-c:a libtwolame` for L2 stereo,
`-c:a mp1` synthesised for the L1 fixture). `*.ref.pcm` is the same ffmpeg
build's own decoder at `s32le` — an independent implementation, never a
runtime dependency. Reference dumps marked `*.mimp3.pcm`/`*.drmp3.pcm` were
decoded offline with `ref_decode*.c` (compiled against minimp3/dr_mp3 above)
for second-reference comparison during bring-up.

## CRC

`crc.rs` implements CRC-16 poly 0x8005, init 0xFFFF, MSB-first, no final
XOR per ISO/IEC 11172-3 §2.4.1.3/§2.4.2.4 (the MPEG audio frame CRC).
Known-answer test: `"123456789"` → `0xAEE7` — the catalog value `0xFEE8`
is CRC-16/BUYPASS (same polynomial, init `0x0000`); pinned in
`src/crc.rs` unit tests.

## Dequantization note

Layer I/II non-grouped requantization follows ISO §2.4.3.2
`s" = (2^nb/(2^nb-1))·(s'" + 2^(-nb+1))` — in raw-code terms
`(2q - 2^nb + 2)/(2^nb - 1)` (the asymmetric +2, matching libmad's `I_sample`,
ffmpeg's `l1_unscale` and mpg123's `(-1<<n)+sample+1`; not the intuitive
midpoint `+1`). Grouped Layer II codes use the C/D pairs of spec Table B.4.
