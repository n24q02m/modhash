//! JPEG decoding for baseline (SOF0/SOF1) and progressive (SOF2) profiles.
//!
//! Part of the `modhash` zero-dependency hashing kit: this crate depends
//! only on `modhash-primitives`, `modhash-math` and `modhash-raster`, so the
//! whole kit resolves without a single registry package.
//!
//! # Scope
//!
//! Decodes JFIF/EXIF-style JPEG streams: 8-bit precision, Huffman entropy
//! coding, 1–3 components, sampling factors up to 2×2 per component
//! (4:4:4, 4:2:2, 4:4:0, 4:2:0 and friends), restart markers, and the full
//! progressive script (spectral selection + successive approximation,
//! interleaved DC scans, EOBRUN). Arithmetic-coded frames (SOF9–SOF11,
//! SOF13–SOF15), lossless frames (SOF3/SOF7/SOF11/SOF15), differential
//! frames (SOF5–SOF7/13–15), 12-bit precision and CMYK output are refused
//! with [`Error::Unsupported`] naming the exact coding process.
//!
//! # Fidelity contract
//!
//! The integer pipeline is a transcription of libjpeg-turbo's `islow`
//! IDCT (jidctint.c), `fancy` chroma upsampling (jdsample.c) and
//! fixed-point YCbCr→RGB (jdcolor.c), so output is **byte-exact** with
//! `libjpeg -dct int -nosmooth` (which is what Pillow produces). The only
//! deliberate divergence is arithmetic on adversarially large
//! coefficients: intermediate sums are `i64`, where libjpeg's `int`
//! accumulators would wrap.
//!
//! `modhash_math::idct2`/`idct2_2d` is *not* used on the decode path: it is
//! an orthonormal DCT-III in `f64` whose rounding differs from JPEG's
//! fixed-point `islow`, so substituting it would break byte-exactness with
//! the reference decoder. It is used in this crate's test-suite as an
//! independent oracle that the transcription stays inside the documented
//! tolerance (see `tests/` and `src/idct.rs`).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod baseline;
mod bits;
mod color;
mod huffman;
mod idct;
mod parser;
mod progressive;
mod upsample;

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};
use modhash_raster::{Gray, Image, Rgb};

use parser::{Frame, Marker, Parser};

/// A decoded JPEG image: single-component frames yield [`Image<Gray>`],
/// three-component frames yield [`Image<Rgb>`].
#[derive(Clone, Debug)]
pub enum Jpeg {
    /// Grayscale (`Image<Gray, u8>`).
    Gray(Image<Gray, u8>),
    /// Color (`Image<Rgb, u8>`).
    Rgb(Image<Rgb, u8>),
}

impl Jpeg {
    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        match self {
            Jpeg::Gray(i) => i.width(),
            Jpeg::Rgb(i) => i.width(),
        }
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        match self {
            Jpeg::Gray(i) => i.height(),
            Jpeg::Rgb(i) => i.height(),
        }
    }
}

/// Decodes a JPEG byte stream into pixels.
///
/// Returns [`Error::Unsupported`] for arithmetic-coded, lossless,
/// differential, hierarchical, 12-bit-precision or CMYK/YCCK files — the
/// message names the coding process — and [`Error::Truncated`] /
/// [`Error::BadValue`] for malformed structure. This function never
/// panics on untrusted input.
pub fn decode(data: &[u8]) -> Result<Jpeg> {
    let mut p = Parser::new(data);
    p.soi()?;

    let mut tables = parser::Tables::default();
    let mut frame: Option<Frame> = None;
    let mut saw_scan = false;
    let mut saw_eoi = false;

    while !saw_eoi {
        let marker = p.next_marker()?;
        match marker {
            Marker::Eoi => saw_eoi = true,
            Marker::App(_) | Marker::Com => {
                let seg = p.segment()?;
                if let Marker::App(14) = marker {
                    parser::parse_app14(seg, &mut tables);
                }
            }
            Marker::Dqt => parser::parse_dqt(p.segment()?, &mut tables)?,
            Marker::Dht => parser::parse_dht(p.segment()?, &mut tables)?,
            Marker::Dri => {
                let seg = p.segment()?;
                if seg.len() < 2 {
                    return Err(Error::truncated("DRI segment", 2, seg.len()));
                }
                tables.restart_interval = u16::from_be_bytes([seg[0], seg[1]]) as usize;
            }
            Marker::Sof(m) => {
                let seg = p.segment()?;
                frame = Some(parser::parse_sof(m, seg)?);
            }
            Marker::Sos => {
                if frame.is_none() {
                    return Err(Error::BadValue("SOS before SOF"));
                }
                let seg = p.segment()?;
                let f = frame.as_mut().expect("checked above");
                if f.progressive {
                    progressive::decode_scan(&mut p, seg, f, &tables)?;
                } else {
                    if saw_scan {
                        return Err(Error::BadValue("second SOS in sequential JPEG"));
                    }
                    baseline::decode_scan(&mut p, seg, f, &tables)?;
                }
                saw_scan = true;
            }
            Marker::Dnl => {
                // DNL carries a late height definition; height==0 was
                // already refused at SOF, so the segment is skippable.
                let _ = p.maybe_segment();
            }
            Marker::Rst(_) => {
                // Stray restart marker between scans: no body, ignore.
            }
            Marker::Dac => {
                let _ = p.segment()?;
                return Err(Error::Unsupported(
                    "arithmetic entropy coding (DAC present)",
                ));
            }
            Marker::Dhp | Marker::Exp | Marker::Jpg | Marker::JpgExtension(_) => {
                let _ = p.maybe_segment();
            }
        }
    }

    let frame = frame.ok_or(Error::BadValue("no frame (SOF) found"))?;
    if !saw_scan {
        return Err(Error::Truncated {
            what: "scan data",
            needed: 1,
            found: 0,
        });
    }
    finish(&frame, &tables)
}

/// Runs IDCT on every decoded block, upsamples components to full
/// resolution and converts color.
fn finish(frame: &Frame, tables: &parser::Tables) -> Result<Jpeg> {
    // Bound all intermediate allocation by the same ceiling the output
    // Image enforces (plus slack for the ×4 coefficient expansion and
    // MCU padding).
    let sample_cap = modhash_raster::MAX_BUFFER_BYTES;
    let total_coefs: usize = frame.comps.iter().map(|c| c.coefs.len()).sum();
    if total_coefs > sample_cap {
        return Err(Error::too_large("coefficient buffers", sample_cap));
    }

    // Decode every block through islow IDCT into the component plane
    // (padded to whole blocks; only `down_w`/`down_h` samples are real).
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(frame.comps.len());
    for comp in frame.comps.iter() {
        let qt = tables.quant[comp.tq as usize]
            .as_ref()
            .ok_or(Error::BadValue("scan uses missing quantization table"))?;
        let pw = comp.blocks_w * 8;
        let ph = comp.blocks_h * 8;
        if pw.checked_mul(ph).is_none_or(|n| n > sample_cap * 4) {
            return Err(Error::too_large("component plane", sample_cap * 4));
        }
        let mut plane = alloc::vec![0u8; pw * ph];
        for by in 0..comp.blocks_h {
            for bx in 0..comp.blocks_w {
                let bi = by * comp.blocks_w + bx;
                let block = &comp.coefs[bi * 64..bi * 64 + 64];
                idct::dequant_idct_into(block, qt, &mut plane, pw, bx * 8, by * 8);
            }
        }
        planes.push(plane);
    }

    // Upsample every component to frame resolution.
    let mut full: Vec<Vec<u8>> = Vec::with_capacity(frame.comps.len());
    for (comp, plane) in frame.comps.iter().zip(planes.iter()) {
        full.push(upsample::to_full_size(
            comp,
            plane,
            comp.blocks_w * 8,
            frame,
        )?);
    }

    color::convert(frame, &full, tables.adobe_transform)
}
