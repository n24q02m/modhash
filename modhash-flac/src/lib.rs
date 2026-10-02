//! FLAC subset decoding: STREAMINFO, constant and verbatim subframes,
//! fixed predictors of orders 0-4, and both Rice residual methods
//! (RFC 9639, formerly the Xiph FLAC format specification).
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! Deliberately outside scope (each surfaces as [`Error::Unsupported`]):
//! LPC subframes (type codes 32-63), which are the only subframe kind a
//! real encoder produces that this crate declines. Ogg-embedded FLAC and
//! ID3-prefixed files are not detected at all: the stream must begin with
//! `fLaC`. The STREAMINFO MD5 is parsed but never verified — the kit has
//! no MD5 primitive — and no check compares decoded length against the
//! declared total, so a stream that ends early decodes what it has.
//!
//! The crate is `no_std` apart from the `alloc` [`Vec`](alloc::vec::Vec)
//! its API returns. Samples are computed in `i64` (a stereo side channel
//! is coded one bit wider than the input) and narrowed to `i32` only
//! after decorrelation, matching the canonical PCM shape `modhash-wav`
//! emits.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod crc;
mod frame;
mod reader;

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::reader::FlacReader;

/// The four magic bytes every native FLAC stream opens with.
const MAGIC: &[u8; 4] = b"fLaC";

/// The fixed STREAMINFO body length in bytes (RFC 9639 section 8.2).
const STREAMINFO_LEN: usize = 34;

/// Ceilings a caller imposes on one decode call.
///
/// [`Limits`] is not optional and there is no silently permissive
/// default: a `constant` subframe turns a handful of input bits into
/// 65535 output samples, so a decoder without a ceiling on what it will
/// produce is a denial-of-service primitive. [`Default`] exists for the
/// common case - a caller hashing one file - and is a *conservative*
/// ceiling, not an unlimited one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Hard ceiling on input consumed, in bytes. An input longer than
    /// this is [`Error::TooLarge`] before a single bit is read. The
    /// default is 64 MiB.
    pub max_input: usize,
    /// Hard ceiling on produced PCM, in bytes (each sample is 4 bytes of
    /// `i32`). Producing more than this is [`Error::TooLarge`], never an
    /// allocation attempt: every allocation derived from stream data is
    /// clamped by this value before it happens. The default is 256 MiB.
    pub max_output: usize,
}

impl Default for Limits {
    /// A conservative ceiling: 64 MiB in, 256 MiB of PCM out (about
    /// 25 minutes of stereo 44.1 kHz audio).
    fn default() -> Self {
        Limits {
            max_input: 64 * 1024 * 1024,
            max_output: 256 * 1024 * 1024,
        }
    }
}

/// The mandatory first metadata block (RFC 9639 section 8.2).
///
/// `sample_rate` may be zero: the format marks STREAMINFO's rate field
/// invalid-at-zero, but real files carry it and the frames still declare
/// their own rate, so the value is surfaced as parsed and frame rates
/// are enforced uniform among themselves instead.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Smallest block size in the stream (samples per channel).
    pub min_block_size: u16,
    /// Largest block size in the stream.
    pub max_block_size: u16,
    /// Smallest frame size in bytes; 0 if unknown.
    pub min_frame_size: u32,
    /// Largest frame size in bytes; 0 if unknown.
    pub max_frame_size: u32,
    /// Sample rate in Hz; 0 means the frames carry it.
    pub sample_rate: u32,
    /// Channel count, 1-8.
    pub channels: u8,
    /// Bits per sample, 4-32.
    pub bits_per_sample: u8,
    /// Total samples per channel; 0 means unknown.
    pub total_samples: u64,
    /// The unverifiable MD5 of the unencoded audio, for provenance only.
    pub md5: [u8; 16],
}

/// Decoded PCM: the samples of every frame, interleaved per channel.
///
/// `channels`, `sample_rate`, `bits_per_sample` and `samples` mirror the
/// `Wav` struct's field set exactly — same types, same interleaving, same
/// sign-extended-integer semantics — so `modhash-audio` consumes both
/// decoders identically. FLAC adds `total_samples`, the 36-bit declared
/// length STREAMINFO carries, because it is provenance a hashing
/// pipeline may want; it is not the decoded length when the format
/// marked it unknown (0) or the stream ended early.
#[derive(Clone, Debug, PartialEq)]
pub struct Flac {
    /// Information from the STREAMINFO block as parsed.
    channels: u16,
    /// Sample rate in Hz, resolved from frames when STREAMINFO said 0.
    sample_rate: u32,
    /// Bits per sample.
    bits_per_sample: u16,
    /// Declared samples per channel, 0 when the stream said "unknown".
    total_samples: u64,
    /// Interleaved samples: `samples[i * channels + c]`, i32-valued.
    samples: Vec<i32>,
}

impl Flac {
    /// Channel count, 1-8.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Bits per sample the stream was coded at, 4-32.
    pub fn bits_per_sample(&self) -> u16 {
        self.bits_per_sample
    }

    /// Samples per channel STREAMINFO declared; 0 means unknown.
    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    /// Interleaved PCM: `samples[i * channels + c]` is channel `c` of
    /// sample `i`, sign-extended into `i32`. Stereo assignments are
    /// already decorrelated; no side channel is ever visible here.
    pub fn samples(&self) -> &[i32] {
        &self.samples
    }

    /// Frames actually decoded (`samples.len() / channels`).
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }
}

/// Parses the `fLaC` signature plus exactly the STREAMINFO block, the
/// only part of a stream callers need before deciding whether to decode.
///
/// The first metadata block must be STREAMINFO (RFC 9639 section 8.1):
/// anything else is [`Error::BadValue`], and a STREAMINFO whose declared
/// length is not 34 bytes is [`Error::Truncated`] or
/// [`Error::BadValue`] accordingly. Later blocks are not examined here.
pub fn decode_streaminfo(input: &[u8]) -> Result<StreamInfo> {
    if input.len() < 4 || &input[..4] != MAGIC {
        return Err(Error::InvalidMagic {
            what: "FLAC signature",
        });
    }
    let header = input
        .get(4..8)
        .ok_or_else(|| Error::truncated("FLAC metadata header", 8, input.len()))?;
    if header[0] & 0x7F != 0 {
        return Err(Error::BadValue(
            "FLAC STREAMINFO must be the first metadata block",
        ));
    }
    let len = (u32::from(header[1]) << 16) | (u32::from(header[2]) << 8) | u32::from(header[3]);
    if usize::try_from(len).ok() != Some(STREAMINFO_LEN) {
        return Err(Error::BadValue("FLAC STREAMINFO length"));
    }
    let body = input
        .get(8..8 + STREAMINFO_LEN)
        .ok_or_else(|| Error::truncated("FLAC STREAMINFO", 8 + STREAMINFO_LEN, input.len()))?;
    Ok(parse_streaminfo(body))
}

/// Decodes a whole native FLAC stream into PCM.
///
/// Layout (RFC 9639 section 8): signature, one or more metadata blocks
/// (STREAMINFO first; every other block is skipped by length, its body
/// uninterpreted), then frames until input ends. The first frame follows
/// immediately after the last metadata block; a byte that is not the
/// start of a valid sync code at that boundary is `Error::InvalidMagic`,
/// never a resync attempt - resynchronizing silently decodes the wrong
/// samples, which is worse than refusing.
///
/// Every frame is cross-checked against STREAMINFO and its own CRC-8 /
/// CRC-16; a declared `total_samples` smaller than the decoded length is
/// `Error::BadValue`. Output accumulates into one `Vec<i32>` capped by
/// [`Limits::max_output`].
pub fn decode(input: &[u8], limits: &Limits) -> Result<Flac> {
    if input.len() > limits.max_input {
        return Err(Error::too_large("FLAC input", limits.max_input));
    }
    if input.len() < 4 || &input[..4] != MAGIC {
        return Err(Error::InvalidMagic {
            what: "FLAC signature",
        });
    }

    // Metadata blocks: 1-bit last flag, 7-bit type, 24-bit length.
    // STREAMINFO is mandatory and first; type 127 is forbidden outright.
    let mut at = 4usize;
    let mut info: Option<StreamInfo> = None;
    let mut first = true;
    loop {
        let header = input
            .get(at..at + 4)
            .ok_or_else(|| Error::truncated("FLAC metadata header", at + 4, input.len()))?;
        at += 4;
        let last = header[0] & 0x80 != 0;
        let ty = header[0] & 0x7F;
        let len = usize::try_from(
            (u32::from(header[1]) << 16) | (u32::from(header[2]) << 8) | u32::from(header[3]),
        )
        .map_err(|_| Error::BadValue("FLAC metadata block length"))?;
        if first {
            if ty != 0 {
                return Err(Error::BadValue(
                    "FLAC STREAMINFO must be the first metadata block",
                ));
            }
            if len != STREAMINFO_LEN {
                return Err(Error::BadValue("FLAC STREAMINFO length"));
            }
            let body = input
                .get(at..at + STREAMINFO_LEN)
                .ok_or_else(|| Error::truncated("FLAC STREAMINFO", at + len, input.len()))?;
            info = Some(parse_streaminfo(body));
        } else if ty == 127 {
            return Err(Error::BadValue("FLAC metadata block type 127"));
        }
        at = at
            .checked_add(len)
            .ok_or(Error::BadValue("FLAC metadata block length"))?;
        if at > input.len() {
            return Err(Error::truncated("FLAC metadata block", at, input.len()));
        }
        first = false;
        if last {
            break;
        }
    }
    let info = info.ok_or(Error::BadValue("FLAC missing STREAMINFO"))?;

    // Frames until the input is exhausted. A remainder of 1-15 bits can
    // never hold a sync code, so it is trailing garbage, not a frame.
    let mut r = FlacReader::new(&input[at..]);
    let mut rate: Option<u32> = (info.sample_rate != 0).then_some(info.sample_rate);
    let cap = limits.max_output / 4;
    let mut samples: Vec<i32> = Vec::new();
    while r.remaining() >= 16 {
        let f = frame::frame(&mut r, &info, rate)?;
        if f.rate != 0 {
            rate = Some(f.rate);
        }
        if f.samples.len() > cap.saturating_sub(samples.len()) {
            return Err(Error::too_large("FLAC decoded PCM", limits.max_output));
        }
        if info.total_samples != 0
            && samples.len() / usize::from(info.channels)
                + f.samples.len() / usize::from(info.channels)
                > info.total_samples as usize
        {
            return Err(Error::BadValue(
                "FLAC stream exceeds declared total samples",
            ));
        }
        for v in f.samples {
            samples.push(
                i32::try_from(v).map_err(|_| Error::BadValue("FLAC sample out of 32-bit range"))?,
            );
        }
    }
    if r.remaining() > 0 {
        return Err(Error::BadValue("FLAC trailing bits after last frame"));
    }

    if samples.is_empty() && info.total_samples != 0 {
        return Err(Error::BadValue("FLAC stream has no frames"));
    }
    Ok(Flac {
        channels: u16::from(info.channels),
        sample_rate: rate.unwrap_or(info.sample_rate),
        bits_per_sample: u16::from(info.bits_per_sample),
        total_samples: info.total_samples,
        samples,
    })
}

/// Reads the 34 STREAMINFO bytes per RFC 9639 section 8.2. `body` is
/// exactly 34 bytes (callers guarantee it), so every field read is
/// in-bounds by construction.
fn parse_streaminfo(body: &[u8]) -> StreamInfo {
    let mut r = FlacReader::new(body);
    // Every read below is bounded by STREAMINFO_LEN == 34 bytes = 272
    // bits; the `ok()`-discarding helper cannot fail, it only keeps the
    // parser expression-shaped.
    let take = |r: &mut FlacReader<'_>, n: u32| r.bits(n, "FLAC STREAMINFO").unwrap_or(0);
    let mut md5 = [0u8; 16];
    let min_block_size = take(&mut r, 16) as u16;
    let max_block_size = take(&mut r, 16) as u16;
    let min_frame_size = take(&mut r, 24) as u32;
    let max_frame_size = take(&mut r, 24) as u32;
    let sample_rate = take(&mut r, 20) as u32;
    let channels = (take(&mut r, 3) as u8) + 1;
    let bits_per_sample = (take(&mut r, 5) as u8) + 1;
    let total_samples = take(&mut r, 36);
    for b in &mut md5 {
        *b = take(&mut r, 8) as u8;
    }
    StreamInfo {
        min_block_size,
        max_block_size,
        min_frame_size,
        max_frame_size,
        sample_rate,
        channels,
        bits_per_sample,
        total_samples,
        md5,
    }
}
