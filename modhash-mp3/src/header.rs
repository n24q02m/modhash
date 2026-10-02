//! The 32-bit MPEG audio frame header.
//!
//! Layout per ISO/IEC 11172-3 §2.4.1.2 / §2.4.2.3: the first 11 bits are the
//! sync word `0x7FF`, then `version` (this crate implements only MPEG-1,
//! code `11`), `layer` (`01` Layer III, `10` Layer II, `11` Layer I), a
//! `protection` bit (0 means a 16-bit CRC follows the header), the bitrate
//! and sampling-rate indices, a padding bit, private bit, mode, mode
//! extension, copyright, original and emphasis.

use crate::tables::{BITRATE_L1, BITRATE_L2, BITRATE_L3, SAMPLE_RATES};
use modhash_primitives::{Error, Result};

/// One of the three MPEG-1 audio layers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Layer I — simplest subband coder, 384 samples per frame.
    I,
    /// Layer II — grouped quantization + scalefactor select, 1152 samples.
    II,
    /// Layer III — hybrid filterbank with Huffman coding, 1152 samples.
    III,
}

/// Channel mode from the header's `mode` field.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Two fully independent channels.
    Stereo,
    /// Layer I/II: shared subbands above a bound. Layer III: intensity and/
    /// or mid-side stereo per `mode_ext`.
    JointStereo,
    /// Two mono programmes carried in one stream.
    DualChannel,
    /// One channel.
    SingleChannel,
}

/// Everything `Header::parse` verifies out of the 32-bit field.
#[derive(Copy, Clone, Debug)]
pub struct Header {
    /// Which layer this frame carries.
    pub layer: Layer,
    /// True when no CRC-16 follows the header (`protection_bit == 1`).
    pub unprotected: bool,
    /// Bitrate in kbit/s.
    pub bitrate_kbps: u32,
    /// Sampling rate in Hz.
    pub sample_rate: u32,
    /// True when one slot of padding was added.
    pub padded: bool,
    /// Channel mode.
    pub mode: Mode,
    /// The 2-bit mode extension (joint-stereo bound for I/II, stereo flags
    /// for III).
    pub mode_ext: u8,
    /// Total byte length of this frame including the header.
    pub frame_bytes: usize,
}

impl Header {
    /// Number of PCM samples this frame yields per channel.
    pub fn samples_per_channel(&self) -> usize {
        match self.layer {
            Layer::I => 384,
            Layer::II | Layer::III => 1152,
        }
    }

    /// Number of audio channels.
    pub fn channels(&self) -> usize {
        match self.mode {
            Mode::SingleChannel => 1,
            _ => 2,
        }
    }

    /// Layer III: joint-stereo means mid-side when `mode_ext` bit 1 set,
    /// intensity when bit 0 set (or both).
    pub fn ms_stereo(&self) -> bool {
        self.mode == Mode::JointStereo && (self.mode_ext & 2) != 0
    }
    /// Layer III intensity stereo flag.
    pub fn intensity_stereo(&self) -> bool {
        self.mode == Mode::JointStereo && (self.mode_ext & 1) != 0
    }

    /// Layer I/II: index of the first jointly-coded subband.
    /// L1 bound = (mode_ext+1)*4; L2 bound is the same formula, clamped to
    /// the subband count the caller is using. (Reference decoders all use
    /// *4 for Layer II as well; the spec text only gives the L1 table.)
    pub fn js_bound(&self, sblimit: usize) -> usize {
        let bound = match self.layer {
            Layer::I | Layer::II => (usize::from(self.mode_ext) + 1) * 4,
            Layer::III => 0,
        };
        bound.min(sblimit)
    }

    /// Parse the four header bytes at the front of `data`.
    ///
    /// `data` must contain at least 4 bytes. Errors:
    /// [`Error::Truncated`] (fewer than 4 bytes), [`Error::BadValue`] for a
    /// non-sync word, a reserved index, or the "free" bitrate format which
    /// needs a variable-length search this decoder does not implement, and
    /// [`Error::Unsupported`] for MPEG-2/2.5 streams.
    pub fn parse(data: &[u8]) -> Result<Header> {
        if data.len() < 4 {
            return Err(Error::Truncated {
                what: "frame header",
                needed: 4,
                found: data.len(),
            });
        }
        let w = (u32::from(data[0]) << 24)
            | (u32::from(data[1]) << 16)
            | (u32::from(data[2]) << 8)
            | u32::from(data[3]);
        if (w >> 21) != 0x7FF {
            return Err(Error::BadValue("sync word"));
        }
        let version = (w >> 19) & 3;
        if version == 0b01 {
            return Err(Error::BadValue("version bits"));
        }
        if version != 0b11 {
            return Err(Error::Unsupported("MPEG-2/2.5 audio"));
        }
        let layer = match (w >> 17) & 3 {
            0b01 => Layer::III,
            0b10 => Layer::II,
            0b11 => Layer::I,
            _ => return Err(Error::BadValue("layer bits")),
        };
        let unprotected = (w >> 16) & 1 == 1;
        let bri = ((w >> 12) & 0xF) as usize;
        if bri == 0xF {
            return Err(Error::BadValue("bitrate index"));
        }
        if bri == 0 {
            return Err(Error::Unsupported("free bitrate format"));
        }
        let sri = ((w >> 10) & 3) as usize;
        if sri == 3 {
            return Err(Error::BadValue("sampling rate index"));
        }
        let bitrate_kbps = match layer {
            Layer::I => BITRATE_L1[bri],
            Layer::II => BITRATE_L2[bri],
            Layer::III => BITRATE_L3[bri],
        };
        let sample_rate = SAMPLE_RATES[sri];
        let padded = (w >> 9) & 1 == 1;
        let mode = match (w >> 6) & 3 {
            0 => Mode::Stereo,
            1 => Mode::JointStereo,
            2 => Mode::DualChannel,
            _ => Mode::SingleChannel,
        };
        let mode_ext = ((w >> 4) & 3) as u8;
        // ISO/IEC 11172-3 §2.4.2.3 frame length formulas.
        let frame_bytes = match layer {
            Layer::I => (12 * bitrate_kbps * 1000 / sample_rate + u32::from(padded)) as usize * 4,
            Layer::II | Layer::III => {
                (144 * bitrate_kbps * 1000 / sample_rate + u32::from(padded)) as usize
            }
        };
        Ok(Header {
            layer,
            unprotected,
            bitrate_kbps,
            sample_rate,
            padded,
            mode,
            mode_ext,
            frame_bytes,
        })
    }
}
