# modhash-mp3 table provenance

All generated tables in `modhash-mp3/src/{tables,hufftab}.rs` are produced by
`lab/mp3/gen_mp3_tables.py` and cross-checked against at least two independent
references before landing.

## Normative source

- `lab/mp3/ref/iso11172-3/ANNEX_AB.txt` — OCR of ISO/IEC 11172-3 Annex A/B
  (Huffman Table 3-B.7, window/transform formulas §2.4.3.4, polyphase §2.4.3.2,
  allocation Tables B.2/B.1, sfb tables B.8).
- `lab/mp3/annex_b.txt` — earlier OCR pass of the same annex.

## Independent implementations cross-checked

| file | role | sha256 (first 16) |
|---|---|---|
| `lab/mp3/ref/minimp3.h` | Lieff minimp3 — nested Huffman tables, sfbtab, scf math | `57e437c5c1f0e8b2` |
| `lab/mp3/ref/mad_huffman.c` | libmad huffman tables | `ab7bbad837fda91d` |
| `lab/mp3/ref/mad_layer3.c` | libmad layer III flow (reorder, imdct, alias) | `e59e003561865caa` |
| `lab/mp3/ref/mpg123_frame.c` | mpg123 side-info/scalefactor conventions | `bd526ac84434579d` |
| `lab/mp3/ref/ff_mpegaudio*.c` | ffmpeg tables + `ff_mpa_l2_select_table` | see dir |
| `lab/mp3/ref/jlayer_SynthesisFilter.java` | jlayer polyphase | see dir |

`gen_mp3_tables.py` parses the spec OCR and walks minimp3's nested tables;
disagreement aborts generation (the script exits non-zero).

## Decoded-reference fixtures

`modhash-mp3/tests/fixtures/*.mp3/.mp2` were encoded offline with
ffmpeg 9.0.1 (`-c:a libmp3lame` for L3, `-c:a libtwolame` for L2 stereo);
`*.ref.pcm` is the same ffmpeg build's own decoder at `s32le` — an
independent implementation, never a runtime dependency.

## CRC

`crc.rs` implements CRC-16 poly 0x8005, init 0xFFFF, MSB-first per
ISO/IEC 11172-3 §2.4.1.3/§2.4.2.4 (the MPEG audio layer CRC).
