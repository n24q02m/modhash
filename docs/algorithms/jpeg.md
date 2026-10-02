# `modhash-jpeg` — JPEG baseline + progressive decoder (ITU T.81)

Scope per design spec §4.5 and the P7 lane:

| In scope | Refused by name |
|---|---|
| SOF0 baseline, SOF1 extended sequential (Huffman) | SOF3/7/11/15 lossless → `Unsupported` |
| SOF2 progressive (spectral selection + successive approximation) | SOF9/10/11 + SOF13/14/15 arithmetic → `Unsupported` |
| Restart markers `FFD0..D7` (DRI) | SOF5/6/7 differential, DHP/EXP hierarchical → `Unsupported` |
| 1 component (gray) / 3 components (YCbCr or RGB) | P≠8 precision, 2 or 4 components (CMYK/YCCK) → `Unsupported` |
| Sampling factors 1..4: 4:4:4, 4:2:2, 4:4:0, 4:2:0, 4:1:1 and other integral ratios | Fractional (non-integral) ratios → `Unsupported` |
| JFIF APP0, EXIF APP1, Adobe APP14 (`transform` 0/1), COM, DNL | APP14 transform=2 (YCCK) → `Unsupported` |
| Multi-table DQT/DHT/DAC-adjacent segments | DAC → `Unsupported("arithmetic entropy coding")` |

Every refusal names the coding process, per the spec's "error out
explicitly" rule; nothing unsupported returns a wrong hash.

## Pipeline (mirrors libjpeg-turbo exactly)

```
SOI → {APPn/COM/DQT/DHT/DRI}* → SOF → [SOS → entropy data]⁺ → EOI
                                     ↓ Huffman decode (canonical
                                       mincode/maxcode/valptr walk),
                                       DC diff + run/size AC or
                                       progressive script
                                     ↓ islow dequantize+IDCT (jidctint.c
                                       transcription, i64 accumulators)
                                     ↓ fancy triangle upsample
                                       (jdsample.c h2v1/h1v2/h2v2)
                                     ↓ fixed-point YCbCr→RGB (jdcolor.c,
                                       SCALEBITS=16 tables)
```

- **Bit order**: entropy data is MSB-first; `FF 00` unstuffing; `FF`
  followed by non-00 is a marker. RSTn between intervals resets DC
  predictions and EOBRUN and re-aligns to a byte boundary.
- **Zigzag**: `ZIGZAG[k]` maps scan position → natural index; DQT values
  are stored de-zigzagged so IDCT reads natural order directly.
- **Progressive**: DC scans may be interleaved (MCU walk); AC scans are
  always `Ns=1` over the true block grid. EOBRUN covers the emitting
  block (`EOBRUN--` at emit). AC refine: `r` counts *still-zero*
  positions, correction bits apply to nonzero coefficients passed, new
  coefficients are `±(1<<Al)`, and an EOB-run refines the band tail.
- **Subsampling**: per-component `down = ceil(frame_dim · f / Fmax)`;
  the stored block grid is MCU-padded (`mcus × h_i/v_i`), matching what
  the entropy stream emits. Triangle filters emit `2·ceil(dim/2)`
  samples — one more than the frame edge on odd sizes — so rows are
  staged at pitch `2·dw` and cropped (libjpeg reads only
  `output_width`; identical math).
- **Color**: Adobe APP14 transform=0 ⇒ RGB passthrough; ids `R`,`G`,`B`
  without APP14 also select RGB; otherwise YCbCr with the fixed-point
  `FIX(1.40200)/1.77200/0.34414/0.71414` tables.
- **Range limiting**: the post-IDCT table is a 1024-entry wraparound
  (`v & 1023`); replicated as a four-branch function — including the
  documented wrap behavior for `|v| ≥ 384` past the level shift, which
  hostile streams can reach.

## Divergences from libjpeg (all deliberate, all documented)

- Accumulators are `i64` where libjpeg uses `int`/`JLONG`: identical
  for every encoder-legal stream; on adversarial magnitudes libjpeg
  wraps (UB-suppressed upstream), this crate does not panic.
- Truncated entropy data is a hard `Truncated` error where libjpeg pads
  zeros and warns (`insufficient_data`). The kit refuses rather than
  emits gray.
- `Ah`/`Al` outside T.81's `Ah−1 == Al` refinement rule decode anyway
  (libjpeg only warns on `s != 1`).

## Conformance

`tests/fixtures/`: 11 files decoded **byte-exact** against Pillow
12.3.0 (libjpeg-turbo) — baseline 4:4:4/4:2:2/4:2:0/4:4:0/gray,
progressive 4:4:4/4:2:0/gray, restart-marker variants, odd widths and
heights. See `tests/fixtures/PROVENANCE.md`. `idct.rs` cross-checks
against `modhash_math::idct2_2d` (orthonormal f64 oracle) within ±2 LSB
over 2k random blocks.
