//! Marker-level JPEG stream parser.
//!
//! JPEG files are a sequence of markers `0xFF xx`. Segments carrying data
//! have a big-endian u16 length that includes itself; entropy-coded data
//! follows each SOS header and runs until the next real marker (with
//! `FF 00` byte stuffing inside the data and `FF D0..D7` restart markers
//! allowed mid-scan).

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

/// Recognized marker classes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    /// Start of image (FFD8) — consumed separately.
    Eoi,
    /// APP0..APP15 application segments; carries the `n` of APPn.
    App(u8),
    /// Comment.
    Com,
    /// Define quantization table(s).
    Dqt,
    /// Define Huffman table(s).
    Dht,
    /// Define restart interval.
    Dri,
    /// Define arithmetic conditioning.
    Dac,
    /// Start of frame; carries the marker code byte.
    Sof(u8),
    /// Start of scan.
    Sos,
    /// Define number of lines (rare, post-scan).
    Dnl,
    /// Restart marker 0..7 (in-stream; parser surfaces it for skipping).
    Rst(u8),
    /// Define hierarchical progression.
    Dhp,
    /// Expand reference component(s).
    Exp,
    /// Reserved JPG marker.
    Jpg,
    /// JPG extension markers FFF0..FFFD apart from JPG itself.
    JpgExtension(u8),
}

/// One frame component as declared by SOF.
pub struct Component {
    /// Component identifier byte (as transmitted).
    pub id: u8,
    /// Horizontal sampling factor.
    pub h: usize,
    /// Vertical sampling factor.
    pub v: usize,
    /// Quantization table selector.
    pub tq: u8,
    /// DC Huffman table selector.
    pub td: usize,
    /// AC Huffman table selector.
    pub ta: usize,
    /// Blocks across in the *stored* grid (MCU-padded when interleaved
    /// decoding is in use).
    pub blocks_w: usize,
    /// Blocks down in the stored grid.
    pub blocks_h: usize,
    /// True downsampled dimensions in samples: `ceil(frame_dim * h/Hmax)`.
    pub down_w: usize,
    /// See [`Component::down_w`], vertical.
    pub down_h: usize,
    /// Quantized coefficients, natural (row-major) order, `64 * nblocks`.
    pub coefs: Vec<i32>,
}

/// Frame header state (SOF).
pub struct Frame {
    /// `true` for SOF2 (progressive); `false` for SOF0/SOF1.
    pub progressive: bool,
    /// Image width X.
    pub width: usize,
    /// Image height Y.
    pub height: usize,
    /// Max horizontal sampling factor.
    pub hmax: usize,
    /// Max vertical sampling factor.
    pub vmax: usize,
    /// MCU grid dimensions for interleaved scans.
    pub mcus_x: usize,
    /// See [`Frame::mcus_x`], vertical.
    pub mcus_y: usize,
    /// Components in scan order of declaration.
    pub comps: Vec<Component>,
}

/// Mutable decoder tables that persist across scans.
#[derive(Default)]
pub struct Tables {
    /// Quantization tables, natural order (`[u16; 64]`) — `None` if unset.
    pub quant: [Option<Vec<u16>>; 4],
    /// DC Huffman tables 0..3.
    pub huff_dc: [Option<crate::huffman::Table>; 4],
    /// AC Huffman tables 0..3.
    pub huff_ac: [Option<crate::huffman::Table>; 4],
    /// Restart interval in MCUs (0 = none).
    pub restart_interval: usize,
    /// Adobe APP14 transform flag: 0 = RGB passthrough, 1 = YCbCr.
    pub adobe_transform: Option<u8>,
}

/// Sequential cursor over the byte stream after SOI.
pub struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    /// Wraps `data`; call [`Parser::soi`] first.
    pub fn new(data: &'a [u8]) -> Self {
        Parser { data, pos: 0 }
    }

    /// Consumes the mandatory FFD8.
    pub fn soi(&mut self) -> Result<()> {
        if self.data.len() < 2 {
            return Err(Error::truncated("SOI marker", 2, self.data.len()));
        }
        if self.data[0] != 0xff || self.data[1] != 0xd8 {
            return Err(Error::InvalidMagic {
                what: "JPEG SOI (FFD8)",
            });
        }
        self.pos = 2;
        Ok(())
    }

    /// Reads the next non-entropy marker: skips fill bytes and any stray
    /// non-marker bytes (T.81 allows padding `FF`s and tolerates garbage).
    ///
    /// Known segment-less markers are classified; anything else is mapped
    /// to [`Marker::JpgExtension`] only if it is in the reserved range, else
    /// returns [`Error::BadValue`].
    pub fn next_marker(&mut self) -> Result<Marker> {
        loop {
            // Find the next 0xFF followed by a non-0x00 byte, skipping
            loop {
                let b = *self
                    .data
                    .get(self.pos)
                    .ok_or(Error::truncated("next marker", 1, 0))?;
                self.pos += 1;
                if b == 0xff {
                    break;
                }
            }
            // Consume fill 0xFFs.
            while self.pos < self.data.len() && self.data[self.pos] == 0xff {
                self.pos += 1;
            }
            let code = *self
                .data
                .get(self.pos)
                .ok_or(Error::truncated("marker code", 1, 0))?;
            if code == 0x00 {
                // 0xFF 0x00 outside entropy data: treat as fill, rescan.
                self.pos += 1;
                continue;
            }
            self.pos += 1;
            return classify(code);
        }
    }

    /// The underlying byte stream.
    pub fn raw(&self) -> &'a [u8] {
        self.data
    }

    /// Byte offset the decoder's entropy reader should start from.
    pub fn offset(&self) -> usize {
        self.pos
    }
    /// Reads the length-prefixed segment body (length includes itself).
    pub fn segment(&mut self) -> Result<&'a [u8]> {
        if self.data.len() - self.pos < 2 {
            return Err(Error::truncated(
                "segment length",
                2,
                self.data.len() - self.pos,
            ));
        }
        let len = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]) as usize;
        if len < 2 {
            return Err(Error::BadValue("segment length < 2"));
        }
        let body = len - 2;
        if self.data.len() - self.pos - 2 < body {
            return Err(Error::truncated(
                "segment body",
                body,
                self.data.len() - self.pos - 2,
            ));
        }
        let seg = &self.data[self.pos + 2..self.pos + 2 + body];
        self.pos += 2 + body;
        Ok(seg)
    }

    /// Like [`Parser::segment`] for markers that may legally have no body
    /// in malformed input: a short read just rewinds and yields nothing.
    pub fn maybe_segment(&mut self) -> Option<&'a [u8]> {
        let save = self.pos;
        match self.segment() {
            Ok(s) => Some(s),
            Err(_) => {
                self.pos = save;
                None
            }
        }
    }
    /// Re-positions the parser after an entropy-coded region ended at
    /// byte offset `pos` (returned by the scan decoders).
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }
}

fn classify(code: u8) -> Result<Marker> {
    Ok(match code {
        0xd8 => return Err(Error::BadValue("unexpected second SOI")),
        0xd9 => Marker::Eoi,
        0x01 => Marker::JpgExtension(0x01), // TEM: standalone, no body
        0xd0..=0xd7 => Marker::Rst(code - 0xd0),
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 | 0xca | 0xcb | 0xcd | 0xce
        | 0xcf => Marker::Sof(code),
        0xc4 => Marker::Dht,
        0xc8 => Marker::Jpg, // JPG reserved
        0xcc => Marker::Dac,
        0xda => Marker::Sos,
        0xdb => Marker::Dqt,
        0xdc => Marker::Dnl,
        0xdd => Marker::Dri,
        0xde => Marker::Dhp,
        0xdf => Marker::Exp,
        0xe0..=0xef => Marker::App(code - 0xe0),
        0xfe => Marker::Com,
        0xf0..=0xfd => Marker::JpgExtension(code),
        _ => return Err(Error::BadValue("unknown marker code")),
    })
}

/// APP14 "Adobe" segment: byte 11 is the transform flag
/// (0 = RGB/CMYK passthrough, 1 = YCbCr, 2 = YCCK).
pub fn parse_app14(seg: &[u8], tables: &mut Tables) {
    if seg.len() >= 12 && &seg[..5] == b"Adobe" {
        tables.adobe_transform = Some(seg[11]);
    }
}

/// DQT: possibly several tables per segment. Values arrive in zigzag
/// order and are de-zigzagged into natural order here.
pub fn parse_dqt(seg: &[u8], tables: &mut Tables) -> Result<()> {
    let mut i = 0usize;
    while i < seg.len() {
        let pq = (seg[i] >> 4) as usize;
        let tq = (seg[i] & 15) as usize;
        if tq > 3 {
            return Err(Error::BadValue("DQT destination > 3"));
        }
        i += 1;
        let mut table = alloc::vec![0u16; 64];
        if pq == 0 {
            if seg.len() - i < 64 {
                return Err(Error::truncated("DQT 8-bit table", 64, seg.len() - i));
            }
            for k in 0..64 {
                table[crate::huffman::ZIGZAG[k] as usize] = seg[i + k] as u16;
            }
            i += 64;
        } else if pq == 1 {
            if seg.len() - i < 128 {
                return Err(Error::truncated("DQT 16-bit table", 128, seg.len() - i));
            }
            for k in 0..64 {
                table[crate::huffman::ZIGZAG[k] as usize] =
                    u16::from_be_bytes([seg[i + 2 * k], seg[i + 2 * k + 1]]);
            }
            i += 128;
        } else {
            return Err(Error::BadValue("DQT precision > 1"));
        }
        tables.quant[tq] = Some(table);
    }
    Ok(())
}

/// DHT: possibly several tables per segment.
pub fn parse_dht(seg: &[u8], tables: &mut Tables) -> Result<()> {
    let mut i = 0usize;
    while i < seg.len() {
        if i + 17 > seg.len() {
            return Err(Error::truncated("DHT header", 17, seg.len() - i));
        }
        let tc = (seg[i] >> 4) as usize;
        let th = (seg[i] & 15) as usize;
        if tc > 1 {
            return Err(Error::BadValue("DHT class > 1"));
        }
        if th > 3 {
            return Err(Error::BadValue("DHT destination > 3"));
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&seg[i + 1..i + 17]);
        i += 17;
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if seg.len() - i < total {
            return Err(Error::truncated("DHT symbols", total, seg.len() - i));
        }
        let symbols = seg[i..i + total].to_vec();
        i += total;
        let table = crate::huffman::Table::build(counts, symbols)?;
        match tc {
            0 => tables.huff_dc[th] = Some(table),
            _ => tables.huff_ac[th] = Some(table),
        }
    }
    Ok(())
}

/// SOF: validates and fills [`Frame`]; rejects unsupported coding
/// processes with names.
pub fn parse_sof(code: u8, seg: &[u8]) -> Result<Frame> {
    let progressive = match code {
        0xc0 | 0xc1 => false, // baseline / extended sequential, Huffman
        0xc2 => true,         // progressive, Huffman
        0xc3 => return Err(Error::Unsupported("lossless (sequential) JPEG")),
        0xc5 => return Err(Error::Unsupported("differential sequential JPEG")),
        0xc6 => return Err(Error::Unsupported("differential progressive JPEG")),
        0xc7 => return Err(Error::Unsupported("differential lossless JPEG")),
        0xc9 => return Err(Error::Unsupported("extended sequential, arithmetic coding")),
        0xca => return Err(Error::Unsupported("progressive, arithmetic coding")),
        0xcb => return Err(Error::Unsupported("lossless, arithmetic coding")),
        0xcd => {
            return Err(Error::Unsupported(
                "differential sequential, arithmetic coding",
            ));
        }
        0xce => {
            return Err(Error::Unsupported(
                "differential progressive, arithmetic coding",
            ));
        }
        0xcf => {
            return Err(Error::Unsupported(
                "differential lossless, arithmetic coding",
            ));
        }
        _ => return Err(Error::BadValue("bad SOF marker code")),
    };

    if seg.len() < 6 {
        return Err(Error::truncated("SOF header", 6, seg.len()));
    }
    let precision = seg[0];
    if precision != 8 {
        return Err(Error::Unsupported("data precision other than 8 bits"));
    }
    let height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
    let width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
    let ncomp = seg[5] as usize;
    if width == 0 {
        return Err(Error::BadValue("frame width 0 (DNL unsupported)"));
    }
    if height == 0 {
        return Err(Error::BadValue("frame height 0 (DNL unsupported)"));
    }
    if !(1..=3).contains(&ncomp) {
        return Err(Error::Unsupported(
            "component count outside 1..3 (CMYK/YCCK)",
        ));
    }
    if ncomp == 2 {
        return Err(Error::Unsupported("two-component JPEG"));
    }
    if seg.len() < 6 + 3 * ncomp {
        return Err(Error::truncated(
            "SOF component list",
            6 + 3 * ncomp,
            seg.len(),
        ));
    }

    let mut comps: Vec<(u8, usize, usize, u8)> = Vec::with_capacity(ncomp);
    let mut hmax = 0usize;
    let mut vmax = 0usize;
    for k in 0..ncomp {
        let id = seg[6 + 3 * k];
        let hv = seg[7 + 3 * k];
        let tq = seg[8 + 3 * k];
        let h = (hv >> 4) as usize;
        let v = (hv & 15) as usize;
        if h == 0 || h > 4 || v == 0 || v > 4 {
            return Err(Error::BadValue("sampling factor outside 1..4"));
        }
        if tq > 3 {
            return Err(Error::BadValue("quantization table selector > 3"));
        }
        hmax = hmax.max(h);
        vmax = vmax.max(v);
        comps.push((id, h, v, tq));
    }

    // MCU grid: ceil(Y / (8*Vmax)) × ceil(X / (8*Hmax)).
    let mcus_x = width.div_ceil(8 * hmax);
    let mcus_y = height.div_ceil(8 * vmax);

    let mut components = Vec::with_capacity(ncomp);
    for &(id, h, v, tq) in &comps {
        // True downsampled sample dimensions.
        let down_w = (width * h).div_ceil(hmax);
        let down_h = (height * v).div_ceil(vmax);
        // Stored block grid: padded to the MCU grid for interleaved
        // decoding (this is what the entropy coder emits).
        let blocks_w = mcus_x * h;
        let blocks_h = mcus_y * v;
        // Allocation ceiling is enforced indirectly: Image::new caps the
        // final buffer, and coefs.len() = blocks*64 is bounded by the
        // same order of magnitude.
        let n = blocks_w
            .checked_mul(blocks_h)
            .and_then(|b| b.checked_mul(64))
            .ok_or(Error::too_large("coefficient buffer", usize::MAX))?;
        if n > (1usize << 31) {
            return Err(Error::too_large("coefficient buffer", 1usize << 31));
        }
        components.push(Component {
            id,
            h,
            v,
            tq,
            td: 0,
            ta: 0,
            blocks_w,
            blocks_h,
            down_w,
            down_h,
            coefs: alloc::vec![0i32; n],
        });
    }

    Ok(Frame {
        progressive,
        width,
        height,
        hmax,
        vmax,
        mcus_x,
        mcus_y,
        comps: components,
    })
}
