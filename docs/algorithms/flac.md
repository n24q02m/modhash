# `modhash-flac` — FLAC subset decoder (RFC 9639)

Scope per design spec §4.5 and the per-crate lane:

| In scope | Refused by name |
|---|---|
| STREAMINFO parse | LPC subframes (type codes 32-63) → `Unsupported` |
| Constant + verbatim subframes | Reserved subframe types → `BadValue` |
| Fixed predictors, orders 0-4 | Reserved residual methods (2,3) → `BadValue` |
| Rice partitions, methods 00 and 01, escapes | Ogg/ID3 framing (must start `fLaC`) → `InvalidMagic` |
| Left/side, right/side, mid/side + 1-8 independent | — |
| CRC-8 header + CRC-16 frame checks | — |

Not done: STREAMINFO MD5 is parsed but never verified (the kit has no
MD5 primitive); decoded length is bounded by `total_samples` only upward
(a stream that decodes *more* than declared is `BadValue`; one that ends
early reports what it decoded). `sample_rate` 0 in STREAMINFO is
surfaced as parsed; the first frame carrying an explicit rate supplies
the value, and later frames must agree.

## Layout decoded

```
"fLaC"
metadata block+ : [last:1][type:7][len:24] body[len]   (STREAMINFO must
    be first, type 0, exactly 34 bytes; type 127 forbidden; all other
    blocks skipped by length)
frame+ :
    header [byte-aligned]:
        sync 0x3FFE:14 · reserved 0:1 · blocking strategy:1
        block size code:4 · sample rate code:4 · channel assignment:4
        sample size code:3 · reserved 0:1
        coded frame/sample number: UTF-8-style, 1-7 bytes, ≤36 bits
        block size extra: 8/16 bits iff code 6/7   (in this order)
        sample rate extra: 8/16/16 bits iff code 12/13/14
        CRC-8 over every header byte above
    subframe per channel:
        pad 0:1 · type:6 · wasted flag:1 · (unary wasted-1)?
        constant:   signed value ×1 at (bps+boost-wasted)
        verbatim:   signed value ×block_size
        fixed o:    warmup ×o, then Rice residual, then predict
        >> wasted   (decoded samples shifted left by wasted bits)
    pad: zero bits to byte boundary (non-zero → BadValue)
    CRC-16 over frame bytes from the first sync byte through the pad
```

Constants: block size code 1→192, 2-5→`576<<(c-2)`, 6/7→extra+1,
8-15→`256<<(c-8)`; rate code 0→STREAMINFO, 1-11→`1000×c`, 12→kHz,
13→Hz, 14→daHz, 15→invalid; channels 0-7→independent, 8-10→stereo
decorrelated, 11-15→reserved; bps code 0→STREAMINFO, 1,2,4,5,6,7→
8,12,16,20,24,32, 3→reserved.

Rice: method 00 = 4-bit params (escape 15), 01 = 5-bit (escape 31);
partition order `p`, partition 0 codes `block>>p − order` residuals,
the rest `block>>p`. Quotient = `q` zero bits + `1`; remainder `param`
bits; zig-zag `u`: even→`u/2`, odd→`−((u+1)/2)`. Escape partition: 5-bit
raw width then verbatim residuals (width 0 = all zeros, legal).

Predictors (residual `r[n]`, reconstructed `s[n]`): order 0 `s=r`;
1 `s=s[-1]+r`; 2 `s=2s[-1]−s[-2]+r`; 3 `s=3s[-1]−3s[-2]+s[-3]+r`;
4 `s=4s[-1]−6s[-2]+4s[-3]−s[-4]+r`. Accumulated in `i128`, narrowed
after decorrelation.

Stereo: left/side `r = l − s`; right/side `l = r + s`; mid/side
`m2 = (m<<1)|(s&1); r = (m2−s)>>1; l = (m2+s)>>1`. The side channel is
coded one bit wider than `bits_per_sample` (bps+1).

## Deviations from the task's fallback table

- CRC-8 (`0x07`) and CRC-16 (`0x8005`) are implemented locally in
  `src/crc.rs`, bitwise: `modhash-primitives` exports CRC-32 only, and
  the broadcast at rebase confirmed the different polynomials stay
  private. Known-answer-checked against CRC-8/SMBUS 0xF4 and
  CRC-16/BUYPASS 0xFEE8 for `"123456789"`.
- The bit reader is `modhash_primitives::BitReader` (the shared
  MSB-first primitive added in 3ddf2a4), wrapped in `src/reader.rs`
  only to keep per-field `Truncated` names and FLAC's `unary`/`signed`/
  `align_zero` reads.
- Output shape mirrors `modhash-wav`: `Flac { channels(): u16,
  sample_rate(): u32, bits_per_sample(): u16, total_samples(): u64,
  samples(): &[i32], frames(): usize }`, interleaved sign-extended i32
  with stereo decorrelation already applied.
- `decode(input, &Limits)` keeps inflate's mandatory-limits shape:
  `max_input` 64 MiB, `max_output` 256 MiB of PCM.
