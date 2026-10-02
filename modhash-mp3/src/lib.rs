//! MPEG-1 audio decoding for Layers I, II and III.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's
//! own path dependencies, so the whole kit resolves without a single
//! registry package.
//!
//! # Scope
//!
//! [`decode`] parses a byte stream as a sequence of MPEG-1 audio frames —
//! an optional ID3v2 tag, then frame headers per ISO/IEC 11172-3 — and
//! decodes every complete frame of Layer I, II or III. MPEG-2/2.5
//! extensions (lower sample rates, LSF scalefactor layouts) are refused
//! with [`Error::Unsupported`]; resynchronisation past a corrupt frame
//! is deliberately *not* attempted (a decoder that resyncs decodes the
//! wrong stream silently).
//!
//! # Canonical form
//!
//! Decoded audio is returned as [`Mp3`]: channel count, sample rate, and
//! interleaved `i32` PCM — one `i32` per sample position
//! (`samples[frame * channels + channel]`), scaled from the decoder's
//! native `[-1, 1)` floats by `2^31`. A Xing/Info VBR header frame
//! yields no PCM (it is information, not audio).
//!
//! # Lay of the land
//!
//! - `header`: the 32-bit frame header, CBR/VBR frame sizing.
//! - `layer12`: Layer I/II subband decoders (allocation, scfsi,
//!   scalefactors, grouped dequantization).
//! - `layer3`: Layer III — side info, bit reservoir, scalefactors,
//!   Huffman requantization, stereo, antialias, IMDCT, overlap.
//! - `synthesis`: the polyphase synthesis filterbank (shared).
//! - `crc`: the CRC-16 that can protect every layer's side info.
//! - `tables`/`hufftab`: generated constants (see `lab/mp3/`).
//!
//! # Errors
//!
//! Every hostile input is a named [`Error`] — `InvalidMagic` when no
//! plausible frame starts the stream, `Unsupported` for MPEG-2/2.5 or
//! layer "reserved", `Truncated` for short data, `BadValue` for
//! semantically invalid fields (allocation 15, block_type 0, CRC
//! mismatch, reservoir under-run). Nothing panics.
//!
//! # Example
//!
//! ```
//! use modhash_mp3::decode;
//! let mp3 = decode(b"\xff\xfb\x90\x64...", &Default::default());
//! assert!(mp3.is_err() || mp3.is_ok()); // signature shape
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![no_std]

extern crate alloc;

mod crc;
mod header;
mod hufftab;
mod layer12;
mod layer3;
mod synthesis;
mod tables;

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

pub use crate::header::{Header, Layer, Mode};
use crate::layer3::{Overlap, Reservoir};
use crate::synthesis::Synthesis;
/// Ceilings a caller imposes on one decode call.
///
/// [`Limits`] is not optional and there is no silently permissive
/// default: a hostile frame count multiplies into unbounded output, so
/// a decoder without a ceiling is a denial-of-service primitive.
/// [`Default`] exists for hashing a file and is conservative.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Hard ceiling on input consumed, in bytes. An input longer than
    /// this is [`Error::TooLarge`] before a single bit is read. The
    /// default is 64 MiB.
    pub max_input: usize,
    /// Hard ceiling on produced PCM, in bytes (each `i32` sample is 4
    /// bytes). The default is 512 MiB.
    pub max_output: usize,
    /// Hard ceiling on decoded frames. The default is 1_000_000 —
    /// about 7 hours of Layer III audio at 44.1 kHz.
    pub max_frames: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_input: 64 << 20,
            max_output: 512 << 20,
            max_frames: 1_000_000,
        }
    }
}

/// The decoded stream: interleaved `i32` PCM plus the stream facts a
/// caller needs before deciding what to do with it.
#[derive(Clone, Debug)]
pub struct Mp3 {
    /// 1 or 2 audio channels.
    pub channels: u16,
    /// Samples per second per channel, as declared in the header.
    pub sample_rate: u32,
    /// Interleaved PCM: `samples[frame * channels + channel]`.
    pub samples: Vec<i32>,
    /// Frames actually decoded (`samples.len() / (channels * 1152)`
    /// for Layers II/III).
    pub frames: usize,
    /// True when a Xing/Info VBR header frame was seen (the stream is
    /// nominally VBR; the decoder handles it via per-frame sizing).
    pub vbr: bool,
}

/// Stream facts parsed from the first valid frame header — cheap probe
/// for callers that need format info before deciding to decode.
#[derive(Copy, Clone, Debug)]
pub struct Mp3Info {
    /// Layer of the first frame.
    pub layer: Layer,
    /// Bitrate of the first frame in kbit/s (VBR streams vary after).
    pub bitrate_kbps: u32,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// 1 or 2 channels.
    pub channels: u16,
}

/// A stateful MPEG audio decoder for `decode`'s frame loop.
///
/// Holds the Layer III bit reservoir and IMDCT overlap plus the
/// polyphase FIFOs — every piece of cross-frame state the format
/// carries. Cheap to create; reuse across `feed` calls.
pub struct Decoder {
    reservoir: Reservoir,
    overlap: Overlap,
    synth: [Synthesis; 2],
}

impl Decoder {
    /// A fresh decoder (zero reservoir, zero overlap, zero FIFOs).
    pub fn new() -> Self {
        Decoder {
            reservoir: Reservoir::new(),
            overlap: Overlap::new(),
            synth: [Synthesis::new(), Synthesis::new()],
        }
    }

    /// Decode one frame (starting at the sync word) into the PCM
    /// accumulator. `frame` must be exactly `header.frame_bytes` long.
    ///
    /// Returns the number of interleaved `i32` samples appended —
    /// `384·ch` for Layer I, `1152·ch` for II/III, `0` for a Xing/Info
    /// header frame.
    pub fn feed(&mut self, header: &Header, frame: &[u8], pcm: &mut Vec<i32>) -> Result<usize> {
        match header.layer {
            Layer::I | Layer::II => {
                let sb = layer12::decode_subbands(header, frame)?;
                Ok(self.synth_frame(header, &sb, pcm))
            }
            Layer::III => {
                // A Xing/Info VBR tag occupies the first frame's ancillary
                // space; it carries stream metadata, not decodable audio
                // data — decoding it would emit garbage PCM. Its bytes are
                // still reservoir stream, though: later frames'
                // main_data_begin may point into them.
                if is_xing_frame(header, frame) {
                    let side = 4
                        + if header.unprotected { 0 } else { 2 }
                        + if header.channels() == 1 { 17 } else { 32 };
                    self.reservoir.push(&frame[side..]);
                    return Ok(0);
                }
                let sb =
                    layer3::decode_frame(header, frame, &mut self.reservoir, &mut self.overlap)?;
                Ok(self.synth_frame(header, &sb, pcm))
            }
        }
    }

    /// Run the polyphase synthesis over one frame's subband output and
    /// append interleaved `i32` PCM.
    fn synth_frame(
        &mut self,
        header: &Header,
        sb: &[[[f32; 36]; 32]; 2],
        pcm: &mut Vec<i32>,
    ) -> usize {
        let nch = header.channels();
        let groups = header.samples_per_channel() / 32; // 12 or 36
        let start = pcm.len();
        // Grow once for the whole frame: a per-sample `resize` would
        // truncate the tail whenever a later channel writes a smaller
        // offset than the length a previous channel already produced.
        pcm.resize(start + groups * 32 * nch, 0);
        for t in 0..groups {
            for ch in 0..nch {
                let mut column = [0f32; 32];
                for (band, c) in column.iter_mut().enumerate() {
                    *c = sb[ch][band][t];
                }
                let mut out = [0f32; 32];
                self.synth[ch].process(&column, &mut out);
                for (i, &s) in out.iter().enumerate() {
                    // Interleave: out[t*32 + i][ch]
                    pcm[start + (t * 32 + i) * nch + ch] = float_to_i32(s);
                }
            }
        }
        groups * 32 * nch
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Float PCM `[-1, 1)` → `i32`, clamped — an out-of-range reconstruction
/// saturates instead of wrapping.
fn float_to_i32(f: f32) -> i32 {
    if f.is_nan() {
        return 0;
    }
    let scaled = (f * 2147483648.0) as i64;
    scaled.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Decode a whole MPEG-1 audio stream.
///
/// Skips an ID3v2 tag at the head, then decodes frames until the input
/// ends. A trailing partial frame (truncated tail) is reported as
/// [`Error::Truncated`]; trailing ID3v1 padding (`TAG` + 128 bytes) is
/// ignored.
pub fn decode(input: &[u8], limits: &Limits) -> Result<Mp3> {
    if input.len() > limits.max_input {
        return Err(Error::too_large("MP3 input", limits.max_input));
    }
    let at = skip_id3v2(input);
    let first = next_header(input, at).ok_or(Error::InvalidMagic {
        what: "MPEG audio sync",
    })?;

    let mut dec = Decoder::new();
    let mut pcm: Vec<i32> = Vec::new();
    let mut frames = 0usize;
    let mut vbr = false;
    let channels = first.channels() as u16;
    let sample_rate = first.sample_rate;

    let mut pos = at;
    while pos < input.len() {
        let Some(h) = next_header(input, pos) else {
            // Trailing tag/padding: ID3v1 is `TAG` + 128 bytes; anything
            // else non-sync at the tail is just ignored slack.
            break;
        };
        if pos + h.frame_bytes > input.len() {
            return Err(Error::Truncated {
                what: "MPEG audio frame",
                needed: pos + h.frame_bytes,
                found: input.len(),
            });
        }
        if frames >= limits.max_frames {
            return Err(Error::too_large("MP3 frame count", limits.max_frames));
        }
        if h.sample_rate != sample_rate || h.channels() as u16 != channels {
            return Err(Error::BadValue("mid-stream format change"));
        }
        let before = pcm.len();
        let frame = &input[pos..pos + h.frame_bytes];
        if is_xing_frame(&h, frame) {
            vbr = true;
        }
        dec.feed(&h, frame, &mut pcm)?;
        // Output ceiling: check the post-frame size, not per sample.
        if pcm.len() * 4 > limits.max_output {
            return Err(Error::too_large("MP3 PCM output", limits.max_output));
        }
        let _ = before;
        frames += 1;
        pos += h.frame_bytes;
    }
    if frames == 0 {
        return Err(Error::InvalidMagic {
            what: "MPEG audio frame",
        });
    }
    Ok(Mp3 {
        channels,
        sample_rate,
        samples: pcm,
        frames,
        vbr,
    })
}

/// Parse just the stream facts from the first valid frame — the probe
/// a caller needs before deciding whether to decode.
pub fn decode_header(input: &[u8]) -> Result<Mp3Info> {
    let at = skip_id3v2(input);
    let h = next_header(input, at).ok_or(Error::InvalidMagic {
        what: "MPEG audio sync",
    })?;
    Ok(Mp3Info {
        layer: h.layer,
        bitrate_kbps: h.bitrate_kbps,
        sample_rate: h.sample_rate,
        channels: h.channels() as u16,
    })
}

/// Parse the 32-bit header at `at`; `None` when the bytes can't be a
/// frame start (wrong sync, MPEG-2/2.5, reserved fields).
fn next_header(input: &[u8], at: usize) -> Option<Header> {
    Header::parse(input.get(at..at + 4)?).ok()
}

/// Skip an ID3v2 tag (`ID3` + 4 size bytes of 7-bit fields) at `pos`.
fn skip_id3v2(input: &[u8]) -> usize {
    if input.len() >= 10 && &input[..3] == b"ID3" {
        let sz = (usize::from(input[6] & 0x7F) << 21)
            | (usize::from(input[7] & 0x7F) << 14)
            | (usize::from(input[8] & 0x7F) << 7)
            | usize::from(input[9] & 0x7F);
        return 10 + sz;
    }
    0
}

/// True when this Layer III frame's first side-info–following bytes carry
/// a Xing or Info VBR tag (`Xing`/`Info` magic at the MPEG-1 offset).
fn is_xing_frame(h: &Header, frame: &[u8]) -> bool {
    if h.layer != Layer::III {
        return false;
    }
    // MPEG-1: tag offset = 4 (hdr) + crc(0|2) + side info (17|32).
    let off = 4 + if h.unprotected { 0 } else { 2 } + if h.channels() == 1 { 17 } else { 32 };
    frame
        .get(off..off + 4)
        .is_some_and(|m| m == b"Xing" || m == b"Info")
}
