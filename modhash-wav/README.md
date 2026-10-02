# `modhash-wav`

RIFF/WAVE decoding for 8/16/24/32-bit PCM and 32/64-bit IEEE float samples.

**Status: implemented.** This file is the convention doc for the crate: the
canonical output form below is what `modhash-audio` and the kit's canonical
hash consume, so it is specified here once rather than re-derived per caller.

## Surface

```rust
pub fn decode(bytes: &[u8]) -> Result<Wav>;

pub struct Wav { /* private */ }
impl Wav {
    pub fn channels(&self) -> u16;
    pub fn sample_rate(&self) -> u32;
    pub fn bits_per_sample(&self) -> u16;      // file encoding width
    pub fn format(&self) -> SampleFormat;      // Pcm | IeeeFloat
    pub fn samples(&self) -> &[i32];           // canonical interleaved PCM
    pub fn frames(&self) -> usize;             // samples.len() / channels
}

pub enum SampleFormat { Pcm, IeeeFloat }
```

## Canonical form

`Wav::samples()` is one `i32` per sample position, interleaved in file order:
`samples[frame * channels + channel]`. Downmix/resample are the consumer's
job (`modhash-audio`), not the codec's.

WAV payload bytes are always little-endian. Integer samples are
sign-extended to `i32` **without rescaling** — a decoded sample equals the
raw file value, which keeps decoding byte-exact and keeps this crate
compatible with `modhash-flac`'s identical convention:

| bits | file form | canonical `i32` |
|------|-----------|-----------------|
| 8    | unsigned `0..=255` | `byte - 128` (`-128..=127`) |
| 16   | signed LE | raw `i16` value |
| 24   | signed LE, 3 bytes | sign-extended (`00 00 80` → `-8_388_608`) |
| 32   | signed LE | raw `i32` value |

Float samples (format tag 3) map NaN → `0`, clamp to `[-1.0, 1.0]`, scale by
2³¹ and round half away from zero: `-1.0` → `i32::MIN`, `+1.0` → `i32::MAX`.

## Parsing rules (load-bearing tolerances)

- Chunks are walked to physical end of input; the RIFF size field is
  advisory and never bounds the walk (stale sizes are common).
- Every odd-sized chunk is followed by one pad byte (RIFF rule); skipping
  it keeps the next header aligned.
- A declared chunk size running past the physical end is
  `Error::Truncated` — a shorter payload is never guessed at.
- The first `fmt ` and first `data` chunk win; either order parses.
  Anything after `data` is not read.
- `block_align` must equal `channels * bits/8` exactly; a mismatch means
  the packing is not the contiguous form this decoder assumes.
- A data length that is not a whole number of frames drops the trailing
  partial frame.
- Format tags other than `1` (PCM) and `3` (IEEE float), and bit depths
  outside the table above, are `Error::Unsupported` — a refusal to guess,
  not a parse failure. ADPCM/A-law/WAVE extensible are real WAV variants
  this crate deliberately does not decode.
- `decode` never panics on any input; truncated structures return
  `Error::Truncated` naming the structure.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0). `no_std` + `alloc` only.
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
