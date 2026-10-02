//! RIFF/WAVE decoding for 8/16/24/32-bit PCM and 32/64-bit IEEE float
//! samples.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! # Scope
//!
//! [`decode`] parses the RIFF container (`RIFF` / `WAVE` header, the `fmt `
//! chunk, the `data` chunk) and converts the payload to the kit's canonical
//! PCM form. Audio format tag `1` (integer PCM) with 8, 16, 24 or 32 bits per
//! sample and tag `3` (IEEE float) with 32 or 64 bits per sample are decoded;
//! every other format tag or bit depth is refused with
//! [`Error::Unsupported`] rather than mis-decoded — ADPCM, A-law, WAVE
//! extensible and friends are real WAV variants this crate deliberately does
//! not implement.
//!
//! # Canonical form
//!
//! Decoded audio is returned as [`Wav`]: channel count, sample rate, and one
//! interleaved [`i32`] per sample position (`samples[frame * channels +
//! channel]`). WAV payload bytes are always little-endian, whatever the host
//! is, and every integer sample is sign-extended to `i32` **without
//! rescaling**, so a decoded sample equals the raw file value:
//!
//! | bits | file form | canonical `i32` |
//! |------|-----------|-----------------|
//! | 8    | unsigned `0..=255` | `byte - 128` (`-128..=127`) |
//! | 16   | signed LE | raw `i16` value |
//! | 24   | signed LE, 3 bytes | sign-extended to `i32` (`0x00_00_80` → `-8_388_608`) |
//! | 32   | signed LE | raw `i32` value |
//!
//! Float samples are first clamped to `[-1.0, 1.0]` (NaN maps to `0`), then
//! scaled by 2³¹ and rounded half away from zero to `i32`, so `-1.0` maps to
//! `i32::MIN` and `+1.0` maps to `i32::MAX`.
//!
//! # Tolerance decisions (documented, load-bearing)
//!
//! - Chunk payloads are padded to even size per RIFF; a chunk declared with
//!   an odd size is followed by one pad byte, which is skipped before the
//!   next header.
//! - The RIFF size field is ignored after the header check; chunks are
//!   walked to physical end of input, so files whose RIFF size is stale but
//!   whose chunks are complete still decode.
//! - A chunk whose declared size runs past the physical end is
//!   [`Error::Truncated`]. A shorter data payload is never guessed at.
//! - A data payload whose length is not a whole number of frames drops the
//!   trailing partial frame — a truncated sample cannot be decoded.
//! - The first `fmt ` and the first `data` chunk win; later repeats are
//!   skipped like any unknown chunk. Chunks after `data` are not read.
//! - `block_align` must equal `channels * bits/8` exactly; a mismatch means
//!   the file's packing is not the contiguous form this decoder assumes.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use modhash_primitives::{Error, Result};

/// WAV audio format tag for integer PCM.
const TAG_PCM: u16 = 1;
/// WAV audio format tag for IEEE float.
const TAG_IEEE_FLOAT: u16 = 3;

/// The sample encoding declared by a file's `fmt ` chunk.
///
/// This is the *input* encoding; [`Wav::samples`] is always the canonical
/// interleaved `i32` form described in the crate docs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// Integer PCM (format tag 1), unsigned at 8 bits, signed otherwise.
    Pcm,
    /// IEEE float (format tag 3), `f32` or `f64` in the file.
    IeeeFloat,
}

/// A decoded RIFF/WAVE file in the kit's canonical PCM form.
#[derive(Clone, Debug, PartialEq)]
pub struct Wav {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    format: SampleFormat,
    /// Interleaved canonical samples: `samples[frame * channels + channel]`.
    samples: Vec<i32>,
}

impl Wav {
    /// Number of interleaved channels (`1` = mono, `2` = stereo).
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Samples per second per channel, as declared in `fmt `.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Bits per sample in the file encoding (`8`, `16`, `24`, `32`, or `64`
    /// for float64); the decoded [`samples`](Self::samples) are always `i32`.
    pub fn bits_per_sample(&self) -> u16 {
        self.bits_per_sample
    }

    /// The file's sample encoding (integer PCM or IEEE float).
    pub fn format(&self) -> SampleFormat {
        self.format
    }

    /// Interleaved canonical samples, one `i32` per sample position in file
    /// order: `samples[frame * channels + channel]`. See the crate docs for
    /// the per-depth conversion table.
    pub fn samples(&self) -> &[i32] {
        &self.samples
    }

    /// Complete frames decoded: `samples.len() / channels`.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }
}

/// Reads a little-endian `u16` at `bytes[at..at+2]`; callers guarantee the
/// range exists.
fn le_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

/// Reads a little-endian `u32` at `bytes[at..at+4]`; callers guarantee the
/// range exists.
fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Sign-extends a 3-byte little-endian integer to `i32`. The third byte is
/// the sign byte; shifting the assembled `u32` left 8 then arithmetic-right
/// propagates it.
fn le_i24(bytes: &[u8], at: usize) -> i32 {
    let raw =
        u32::from(bytes[at]) | (u32::from(bytes[at + 1]) << 8) | (u32::from(bytes[at + 2]) << 16);
    ((raw << 8) as i32) >> 8
}

/// Decodes a RIFF/WAVE byte string to the canonical PCM form.
///
/// Returns [`Error::InvalidMagic`] when the RIFF or WAVE signature is wrong,
/// [`Error::Unsupported`] for format tags and bit depths outside the
/// implemented set, [`Error::BadValue`] for structurally broken or missing
/// required parts, and [`Error::Truncated`] when a declared structure runs
/// past the physical end of `bytes`. Never panics on any input.
///
/// # Errors
///
/// See the variant list above; every variant carries the name of the field
/// or chunk at fault.
pub fn decode(bytes: &[u8]) -> Result<Wav> {
    // RIFF header: "RIFF" <u32 size> "WAVE". The size field is advisory
    // (stale sizes are common in the wild); chunk walking is bounded by the
    // physical input instead.
    if bytes.len() < 12 {
        return Err(Error::truncated("riff header", 12, bytes.len()));
    }
    if &bytes[0..4] != b"RIFF" {
        return Err(Error::InvalidMagic {
            what: "riff signature",
        });
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(Error::InvalidMagic {
            what: "wave form type",
        });
    }

    let mut fmt: Option<Fmt> = None;
    let mut data: Option<&[u8]> = None;

    // Chunk walk: each header is 8 bytes, then `size` payload bytes, then a
    // pad byte iff `size` is odd. `pos + 8 + size` cannot wrap: the `size >
    // rest` branch returns first, so `size <= rest` holds on the add path,
    // and `pos + 8 + size <= bytes.len()`. `pos` only ever grows.
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = le_u32(bytes, pos + 4) as usize;
        let rest = bytes.len() - (pos + 8);
        if size > rest {
            return Err(Error::truncated("riff chunk payload", size, rest));
        }
        let payload = &bytes[pos + 8..pos + 8 + size];
        if id == b"fmt " {
            if fmt.is_none() {
                fmt = Some(parse_fmt(payload)?);
            }
        } else if id == b"data" && data.is_none() {
            data = Some(payload);
        }
        if fmt.is_some() && data.is_some() {
            // Samples live in the first data chunk; nothing after it
            // affects the decoded output, so the walk stops once both
            // required chunks are captured. That also tolerates a ragged
            // tail appended after the audio.
            break;
        }
        // RIFF pads every odd-sized chunk to even; skipping the pad byte is
        // what keeps the next header aligned.
        pos += 8 + size + (size & 1);
    }

    let fmt = fmt.ok_or(Error::BadValue("missing fmt chunk"))?;
    let data = data.ok_or(Error::BadValue("missing data chunk"))?;
    decode_samples(&fmt, data)
}

/// The fields of a `fmt ` chunk that decide the decoding path.
struct Fmt {
    format: SampleFormat,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    /// Bytes per interleaved frame: `channels * bits_per_sample / 8`.
    frame_size: usize,
}

/// Parses the fixed 16-byte WAVEFORMAT prefix of a `fmt ` chunk. Extra
/// extension bytes (WAVE extensible carries 22+ more) are ignored after the
/// format tag check, because tags other than 1 and 3 are refused before any
/// extension field could matter.
fn parse_fmt(payload: &[u8]) -> Result<Fmt> {
    if payload.len() < 16 {
        return Err(Error::truncated("fmt chunk", 16, payload.len()));
    }
    let tag = le_u16(payload, 0);
    let channels = le_u16(payload, 2);
    let sample_rate = le_u32(payload, 4);
    let block_align = le_u16(payload, 12);
    let bits = le_u16(payload, 14);

    let format = match tag {
        TAG_PCM => SampleFormat::Pcm,
        TAG_IEEE_FLOAT => SampleFormat::IeeeFloat,
        _ => {
            return Err(Error::Unsupported(
                "wav audio format tag (only PCM=1 and IEEE float=3 are decoded)",
            ));
        }
    };
    let legal_bits = match format {
        SampleFormat::Pcm => matches!(bits, 8 | 16 | 24 | 32),
        SampleFormat::IeeeFloat => matches!(bits, 32 | 64),
    };
    if !legal_bits {
        return Err(Error::Unsupported(
            "wav bit depth (PCM decodes 8/16/24/32, float decodes 32/64)",
        ));
    }
    if channels == 0 {
        return Err(Error::BadValue("zero channels"));
    }
    if sample_rate == 0 {
        return Err(Error::BadValue("zero sample rate"));
    }
    // `channels` is at most 65535 and `bits/8` at most 8, so this product
    // cannot wrap a usize on any supported target.
    let frame_size = usize::from(channels) * usize::from(bits / 8);
    if usize::from(block_align) != frame_size {
        return Err(Error::BadValue(
            "block_align differs from channels * bits_per_sample / 8",
        ));
    }
    Ok(Fmt {
        format,
        channels,
        sample_rate,
        bits_per_sample: bits,
        frame_size,
    })
}

/// Converts a `data` payload to canonical interleaved `i32` samples. Whole
/// frames only: a trailing partial frame is dropped because one missing byte
/// makes every later sample in that frame unrepresentable.
fn decode_samples(fmt: &Fmt, data: &[u8]) -> Result<Wav> {
    let channels = usize::from(fmt.channels);
    let bytes_per_sample = fmt.frame_size / channels;
    // Load-bearing clamp: `whole` bounds decoding to complete frames. The
    // reads below are indexed, not iterator-bounded, so decoding to the raw
    let whole = data.len() - data.len() % fmt.frame_size;
    let frames = whole / fmt.frame_size;
    let mut samples = Vec::with_capacity(frames.saturating_mul(channels));
    let mut off = 0usize;
    while off < whole {
        for ch in 0..channels {
            let at = off + ch * bytes_per_sample;
            let s = &data[at..at + bytes_per_sample];
            let sample = match fmt.format {
                SampleFormat::Pcm => match fmt.bits_per_sample {
                    8 => i32::from(s[0]) - 128,
                    16 => i32::from(i16::from_le_bytes([s[0], s[1]])),
                    24 => le_i24(s, 0),
                    // bits 32 is the only remaining legal PCM depth.
                    _ => i32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                },
                SampleFormat::IeeeFloat => {
                    if fmt.bits_per_sample == 32 {
                        float_to_i32(f64::from(f32::from_le_bytes([s[0], s[1], s[2], s[3]])))
                    } else {
                        float_to_i32(f64::from_le_bytes([
                            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
                        ]))
                    }
                }
            };
            samples.push(sample);
        }
        off += fmt.frame_size;
    }
    Ok(Wav {
        channels: fmt.channels,
        sample_rate: fmt.sample_rate,
        bits_per_sample: fmt.bits_per_sample,
        format: fmt.format,
        samples,
    })
}

/// Canonical float mapping: NaN becomes `0`, the rest is clamped to
/// `[-1.0, 1.0]`, scaled by 2³¹, and rounded half away from zero. The ±0.5
/// nudge plus the truncating `as` cast is exactly round-half-away-from-zero,
/// and `f64 as i32` saturates (since Rust 1.45), so the `+1.0` endpoint lands
/// on `i32::MAX` and `-1.0` on `i32::MIN` without a wider intermediate.
/// Written this way because `f64::round` is not in `core` at the workspace
/// MSRV.
fn float_to_i32(x: f64) -> i32 {
    if x.is_nan() {
        return 0;
    }
    let scaled = x.clamp(-1.0, 1.0) * 2_147_483_648.0;
    if scaled >= 0.0 {
        (scaled + 0.5) as i32
    } else {
        (scaled - 0.5) as i32
    }
}
