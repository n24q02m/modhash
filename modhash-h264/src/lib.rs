//! H.264/AVC **baseline-profile** decoding: Annex-B NAL splitting,
//! SPS/PPS, I- and P-slice macroblock layers, CAVLC entropy decoding,
//! the 4x4 integer transform (plus the Intra-16x16 luma-DC and chroma-DC
//! Hadamard transforms), intra prediction (4x4 and 16x16 luma, 8x8
//! chroma), P-slice motion compensation with quarter-sample luma
//! (6-tap) and eighth-sample chroma interpolation, and the in-loop
//! deblocking filter.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in
//! this workspace builds without a single registry package, so the
//! whole kit resolves offline.
//!
//! # Scope
//!
//! The decoder accepts Baseline-profile streams (`profile_idc` 66) and
//! the Baseline feature subset of Constrained Baseline (66 with the
//! constraint flags), Main (77) and Extended (88) — Main/Extended are
//! parsed because their streams frequently exercise no feature beyond
//! CAVLC + I/P slices. Syntax elements outside that subset surface as
//! [`Error::Unsupported`] with the offending feature named:
//!
//! * `entropy_coding_mode_flag` (CABAC), B/SP/SI slices, `transform_8x8`
//!   (High-profile 8x8 transform), `chroma_format_idc != 1` and every
//!   High+ `profile_idc` (100, 110, 122, 244, 44, 83, 86, 118, 128)
//!   plus unknown values, named by profile name;
//! * field pictures, MBAFF and `frame_mbs_only_flag == 0` altogether;
//! * FMO (`num_slice_groups_minus1 > 0`), arbitrary slice order and
//!   slice groups maping is rejected;
//! * `redundant_pic_cnt_present_flag`, data partitioning (DPA/B/C NALs);
//! * long-term reference marking (MMCO 4/6) and `mmco5` gaps: streams
//!   that mark references long-term error out rather than guess.
//!
//! The stream must be Annex-B framed; `avcC` length-prefixed NALs are
//! `modhash_mp4`'s demux concern, not this crate's.
//!
//! Decoding is deterministic and single-threaded by construction; the
//! output is the reconstructed picture as decoded, with frame cropping
//! applied per SPS.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod cabac;
mod cabac_tables;
mod cavlc;
mod deblock;
mod decoder;
mod dpb;
mod golomb;
mod inter;
mod intra;
mod mb;
mod nal;
mod pps;
mod slice;
mod sps;
mod tables;
mod transform;

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

pub use decoder::Decoder;

/// Decodes one Annex-B byte stream into its pictures in presentation
/// order with the default [`Limits`].
///
/// `Err` names the first structural fault; truncation inside a NAL is
/// `Error::Truncated`, a syntax element outside its table is
/// `Error::BadValue`, and out-of-scope features (CABAC, B/SP/SI
/// slices, field pictures, FMO, slice data partitioning, profiles
/// above the baseline/main CAVLC subset) are `Error::Unsupported`.
pub fn decode(stream: &[u8]) -> Result<Vec<Frame>> {
    decode_with_limits(stream, &Limits::default())
}

/// [`decode`] with an explicit [`Limits`].
pub fn decode_with_limits(stream: &[u8], limits: &Limits) -> Result<Vec<Frame>> {
    if stream.len() > limits.max_input {
        return Err(Error::too_large("h264 input", limits.max_input));
    }
    let mut dec = Decoder::new(*limits);
    dec.push_stream(stream)
}
pub use sps::{Profile, Sps};

/// One decoded output picture, 8-bit 4:2:0, after SPS cropping.
///
/// `stride`-free layout: each plane is `width*height` samples for `y`
/// and `(width/2)*(height/2)` for `cb`/`cr`, row-major, no padding.
/// Cropping (SPS `frame_cropping_*`) is applied so the dimensions are
/// the *displayed* size, which for the odd-width cases the conformance
/// suite probes can differ from the coded size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Displayed width in luma samples.
    pub width: u32,
    /// Displayed height in luma samples.
    pub height: u32,
    /// Luma plane, `width * height` bytes.
    pub y: Vec<u8>,
    /// Cb plane, `(width/2) * (height/2)` bytes.
    pub cb: Vec<u8>,
    /// Cr plane, `(width/2) * (height/2)` bytes.
    pub cr: Vec<u8>,
}

/// Ceilings a caller imposes on one decode call.
///
/// [`Limits`] is not optional: H.264's parse tables let a two-line
/// bitstream demand a gigapixel frame buffer, so decoding without a
/// ceiling is a denial-of-service primitive. [`Default`] covers the
/// kit's hashing use — streams up to 64 MiB producing pictures up to
/// 8192x8192 and at most 1024 pictures — and is *not* a permissive
/// mode: exceeding a bound returns [`Error::TooLarge`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Hard ceiling on input consumed, in bytes. Default 64 MiB.
    pub max_input: usize,
    /// Hard ceiling on `width * height` of any one picture in luma
    /// samples (the coded size, before cropping). Default 33,554,432
    /// (8192x4096, level 5.2 territory).
    pub max_luma_samples: usize,
    /// Hard ceiling on the number of frames one [`decode`] call
    /// produces. Default 1024.
    pub max_frames: usize,
    /// Hard ceiling on `max_num_ref_frames` honoured in the DPB; the
    /// stream's own cap still applies (so a level-4 stream is limited
    /// by the smaller of the two). Default 16.
    pub max_refs: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_input: 64 * 1024 * 1024,
            max_luma_samples: 8192 * 4096,
            max_frames: 1024,
            max_refs: 16,
        }
    }
}
