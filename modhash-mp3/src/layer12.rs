//! Layers I and II: subband coders without spectral transform.
//!
//! Frame layout after the 4-byte header (ISO/IEC 11172-3 §2.4.1/§2.4.2):
//! an optional 16-bit CRC, then allocation (4 bits per subband for I,
//! `nbal` bits from the selected B.2 table for II), the scfsi section
//! (Layer II only), scalefactors (6 bits each), and finally the coded
//! samples — 12 samples per subband for Layer I, 3×12 grouped for
//! Layer II.
//!
//! CRC coverage per spec §2.4.1.3/§2.4.2.4 and confirmed bit-for-bit by
//! libmad's `mad_layer_I`/`mad_layer_II`: header bytes 2..4 plus the
//! allocation for Layer I; header bytes 2..4 plus allocation *and* scfsi
//! for Layer II. The Layer II protected span ends mid-byte, which is why
//! [`crate::crc::Crc16`] accumulates bit-wise.
//!
//! Section ordering is `subband` outermost, `channel` innermost in both
//! allocation and scfsi; for the scalefactor and sample sections the
//! subband loop is outermost too (spec syntax tables; mpg123 and libmad
//! agree — an earlier draft of this module walked channel-outermost and
//! mis-parsed every stereo frame).
//!
//! The dequantizer normalizes to `[-1, 1)` at the midpoint: `q` coded with
//! `b` bits becomes `(2q + 1 - 2^b) / (2^b - 1)` for both layers, then
//! multiplied by `SF_TABLE[scalefactor]`. Layer II grouped codes unpack
//! least-significant first: `c mod n`, `c/n mod n`, `c/n²`.
//!
//! Joint-stereo layout (both layers): subbands below the mode-extension
//! bound carry per-channel everything; subbands at and above it share one
//! allocation and one sample stream but still carry per-channel
//! scalefactors — the shared code is scaled independently per channel.

use crate::crc::Crc16;
use crate::header::{Header, Layer, Mode};
use crate::tables::{ALLOC_TABLES, QUANT_BITS, QUANT_STEPS, SF_TABLE};
use modhash_primitives::{BitReader, Error, Result};

/// `sblimit` for each compact allocation table (B.2a..d).
const L2_SBLIMIT: [usize; 4] = [27, 30, 8, 12];

/// Pick the Layer II allocation table — the formula shared by libmad's
/// `mad_layer_II` and ffmpeg's `ff_mpa_l2_select_table` (ISO §2.4.1.6
/// prescribes the same split by bitrate-per-channel and sampling rate).
fn l2_table_index(h: &Header) -> usize {
    let per_ch = h.bitrate_kbps / h.channels() as u32;
    // ffmpeg's `ff_mpa_l2_select_table` verbatim (MPEG-1 branch):
    // table a for high rate / any 48kHz, b for >=96kbit/ch elsewhere,
    // c for <=48kbit/ch except 32kHz, d otherwise.
    if (h.sample_rate == 48000 && per_ch >= 56) || (56..=80).contains(&per_ch) {
        0
    } else if h.sample_rate != 48000 && per_ch >= 96 {
        1
    } else if h.sample_rate != 32000 && per_ch <= 48 {
        2
    } else {
        3
    }
}

/// Non-grouped dequantizer shared by both layers — ISO/IEC 11172-3
/// §2.4.3.2 `s" = (2^nb/(2^nb-1))·(s'" + 2^(-nb+1))` with `s'"` the
/// offset-binary code shifted to two's-complement; in raw-code terms
/// `s" = (2q - 2^nb + 2)/(2^nb - 1)` (libmad I_sample, ffmpeg
/// l1_unscale and mpg123's `(-1<<n) + sample + 1` all agree; the
/// asymmetric +2 is spec-exact, not the midpoint's +1).
fn unquant(code: u32, bits: u32) -> f32 {
    let levels = (1u32 << bits) as f32;
    (2.0 * code as f32 + 2.0 - levels) / (levels - 1.0)
}

/// Layer II grouped quantizer midpoint formula (Table 3-B.4 `C/D`).
fn l2_group_dequant(c: u32, steps: u32) -> f32 {
    (2.0 * c as f32 + 1.0) / steps as f32 - 1.0
}

#[allow(clippy::needless_range_loop)] // bit-field indices, not data iteration
/// Decode the requantized subband samples of one Layer I or II frame into
/// `out[ch][subband][sample]` — 12 samples per subband for L1, 36 for L2.
///
/// `frame` starts at the sync word: the CRC covers header bytes 2..4, so
/// the parse needs the header as well as the body.
pub(crate) fn decode_subbands(h: &Header, frame: &[u8]) -> Result<[[[f32; 36]; 32]; 2]> {
    let nch = h.channels();
    let joint = h.mode == Mode::JointStereo;
    let body = &frame[4..];
    let mut r = BitReader::new(body);
    let mut out = [[[0.0f32; 36]; 32]; 2];
    let mut crc = Crc16::new();
    crc.bytes(&frame[2..4]);

    // CRC word: occupies the first two body bytes when protection is on.
    // It covers header bytes 2..4 plus the allocation (plus scfsi for
    // Layer II) — i.e. data that physically FOLLOWS the field.
    let crc_target = if h.unprotected {
        None
    } else {
        Some(r.bits(16)? as u16)
    };

    // ---- allocation
    let mut alloc = [[0u8; 32]; 2];
    let mut qbits = [[0i8; 32]; 2]; // L1 coded bits per sample
    let mut qidx = [[0u8; 32]; 2]; // L2 quantization class index
    let sblimit;
    let bound; // first jointly-coded subband (== sblimit when not joint)
    if h.layer == Layer::I {
        sblimit = 32;
        bound = if joint { h.js_bound(32) } else { 32 };
        for sb in 0..bound {
            for ch in 0..nch {
                let a = r.bits(4)? as u8;
                if a == 15 {
                    return Err(Error::BadValue("layer I allocation"));
                }
                crc.field(u64::from(a), 4);
                alloc[ch][sb] = a;
                qbits[ch][sb] = if a > 0 { a as i8 + 1 } else { 0 };
            }
        }
        for sb in bound..32 {
            let a = r.bits(4)? as u8;
            if a == 15 {
                return Err(Error::BadValue("layer I allocation"));
            }
            crc.field(u64::from(a), 4);
            alloc[0][sb] = a;
            alloc[1][sb] = a;
            let b = if a > 0 { a as i8 + 1 } else { 0 };
            qbits[0][sb] = b;
            qbits[1][sb] = b;
        }
    } else {
        let t = l2_table_index(h);
        sblimit = L2_SBLIMIT[t];
        bound = if joint { h.js_bound(sblimit) } else { sblimit };
        let tab = ALLOC_TABLES[t];
        let mut j = 0usize;
        for sb in 0..bound {
            let nbal = tab[j] as usize;
            for ch in 0..nch {
                let code = r.bits(nbal)? as usize;
                crc.field(code as u64, nbal);
                alloc[ch][sb] = code as u8;
                if code > 0 {
                    qidx[ch][sb] = tab[j + code];
                }
            }
            j += 1 << nbal;
        }
        for sb in bound..sblimit {
            let nbal = tab[j] as usize;
            let code = r.bits(nbal)? as usize;
            crc.field(code as u64, nbal);
            alloc[0][sb] = code as u8;
            alloc[1][sb] = code as u8;
            if code > 0 {
                qidx[0][sb] = tab[j + code];
                qidx[1][sb] = tab[j + code];
            }
            j += 1 << nbal;
        }
    }

    // ---- Layer II scfsi: 2 bits per allocated subband, subband-outer /
    // channel-inner, and covered by the CRC.
    let mut scfsi = [[0u8; 32]; 2];
    if h.layer == Layer::II {
        for sb in 0..sblimit {
            for ch in 0..nch {
                if alloc[ch][sb] != 0 {
                    let v = r.bits(2)? as u8;
                    crc.field(u64::from(v), 2);
                    scfsi[ch][sb] = v;
                }
            }
        }
    }

    if let Some(target) = crc_target {
        if crc.finish() != target {
            return Err(Error::BadValue("crc"));
        }
    }

    // ---- scalefactors: L1 keeps one per subband; L2 keeps three per
    // subband gated by scfsi. Read order: subband outer, channel inner,
    // and each channel's factor is fully read before the next subband
    // even in the joint region (shared alloc, per-channel scalefactors).
    let mut sf3 = [[[0u8; 3]; 32]; 2];
    if h.layer == Layer::II {
        for sb in 0..sblimit {
            for ch in 0..nch {
                if alloc[ch][sb] == 0 {
                    continue;
                }
                match scfsi[ch][sb] {
                    0 => {
                        for p in &mut sf3[ch][sb] {
                            *p = r.bits(6)? as u8;
                        }
                    }
                    1 => {
                        sf3[ch][sb][0] = r.bits(6)? as u8;
                        sf3[ch][sb][2] = r.bits(6)? as u8;
                        sf3[ch][sb][1] = sf3[ch][sb][0];
                    }
                    2 => {
                        let s = r.bits(6)? as u8;
                        sf3[ch][sb] = [s, s, s];
                    }
                    _ => {
                        sf3[ch][sb][0] = r.bits(6)? as u8;
                        sf3[ch][sb][2] = r.bits(6)? as u8;
                        sf3[ch][sb][1] = sf3[ch][sb][2];
                    }
                }
            }
        }
    } else {
        for sb in 0..32 {
            for ch in 0..nch {
                if alloc[ch][sb] != 0 {
                    let s = r.bits(6)? as u8;
                    if s == 63 {
                        return Err(Error::BadValue("scalefactor"));
                    }
                    sf3[ch][sb] = [s, s, s];
                }
            }
        }
    }

    // ---- samples
    if h.layer == Layer::I {
        // 12 sample groups; within a group the subband loop is outermost.
        for s in 0..12 {
            for sb in 0..bound {
                for ch in 0..nch {
                    let b = qbits[ch][sb];
                    if b <= 0 {
                        continue;
                    }
                    let q = r.bits(b as usize)? as u32;
                    out[ch][sb][s] = unquant(q, b as u32) * SF_TABLE[sf3[ch][sb][0] as usize];
                }
            }
            for sb in bound..32 {
                let b = qbits[0][sb];
                if b <= 0 {
                    continue;
                }
                let q = r.bits(b as usize)? as u32;
                let f = unquant(q, b as u32);
                for ch in 0..nch {
                    out[ch][sb][s] = f * SF_TABLE[sf3[ch][sb][0] as usize];
                }
            }
        }
    } else {
        // 12 granules of 3 samples; granules 4p..4p+3 use scalefactor
        // part p.
        for part in 0..3usize {
            for g in 0..4usize {
                let base = part * 12 + g * 3;
                for sb in 0..bound {
                    for ch in 0..nch {
                        if alloc[ch][sb] == 0 {
                            continue;
                        }
                        read_three(&mut r, qidx[ch][sb], |k, f| {
                            out[ch][sb][base + k] = f * SF_TABLE[sf3[ch][sb][part] as usize];
                        })?;
                    }
                }
                for sb in bound..sblimit {
                    if alloc[0][sb] == 0 {
                        continue;
                    }
                    read_three(&mut r, qidx[0][sb], |k, f| {
                        for ch in 0..nch {
                            out[ch][sb][base + k] = f * SF_TABLE[sf3[ch][sb][part] as usize];
                        }
                    })?;
                }
            }
        }
    }

    Ok(out)
}

/// Read the three samples of one Layer II granule and hand their
/// normalized values to `f(k, value)` in order.
fn read_three(r: &mut BitReader<'_>, qidx: u8, mut f: impl FnMut(usize, f32)) -> Result<()> {
    let qi = qidx as usize;
    let bits = QUANT_BITS[qi];
    if bits < 0 {
        let n = QUANT_STEPS[qi];
        let c = r.bits((-bits) as usize)? as u32;
        f(0, l2_group_dequant(c % n, n));
        f(1, l2_group_dequant((c / n) % n, n));
        f(2, l2_group_dequant(c / (n * n), n));
    } else {
        for k in 0..3 {
            let q = r.bits(bits as usize)? as u32;
            f(k, unquant(q, bits as u32));
        }
    }
    Ok(())
}
