//! Frame and subframe decoding (RFC 9639 sections 9.2 and 9.3).
//!
//! One frame is: a byte-aligned header (sync, coded sizes, a UTF-8-style
//! coded number, a CRC-8), then one subframe per channel, then zero
//! padding to the next byte boundary and a CRC-16 over everything since
//! the first sync byte.
//!
//! Implemented subframes: `constant`, `verbatim`, and the fixed predictors
//! of orders 0-4 whose residuals are Rice-coded (either 4-bit or 5-bit
//! parameter method, escaped partitions included). LPC subframes and the
//! reserved type codes are refused per the workspace codec scope.

use alloc::vec;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::StreamInfo;
use crate::crc::{crc8, crc16};
use crate::reader::FlacReader;

/// How a frame's channels are stored, from the 4-bit channel assignment
/// field of the frame header (RFC 9639 section 9.2.6).
#[derive(Copy, Clone)]
pub(crate) enum ChannelMap {
    /// 1-8 independent channels, coded independently at `bits_per_sample`.
    Independent(u8),
    /// Channel 0 is left, channel 1 is `left - right` at bps + 1.
    LeftSide,
    /// Channel 0 is `left - right` at bps + 1, channel 1 is right.
    RightSide,
    /// Channel 0 is `floor((left + right) / 2)`, channel 1 is `left -
    /// right` at bps + 1.
    MidSide,
}

impl ChannelMap {
    /// The channel count a map produces: always 1-8.
    fn count(self) -> u8 {
        match self {
            ChannelMap::Independent(n) => n,
            ChannelMap::LeftSide | ChannelMap::RightSide | ChannelMap::MidSide => 2,
        }
    }

    /// The channel index coded one bit wider than `bits_per_sample`, if
    /// this map carries a side channel.
    fn side_channel(self) -> Option<u8> {
        match self {
            ChannelMap::Independent(_) => None,
            // Side is stored in subframe 1 for left/side and mid/side,
            // in subframe 0 for right/side.
            ChannelMap::LeftSide | ChannelMap::MidSide => Some(1),
            ChannelMap::RightSide => Some(0),
        }
    }
}

/// What one decoded frame contributes to the stream.
pub(crate) struct FrameOut {
    /// Interleaved PCM after stereo decorrelation, `i64` until the caller
    /// narrows to the output width.
    pub(crate) samples: Vec<i64>,
    /// The rate the frame resolved to. The first frame may carry the
    /// value STREAMINFO left zero; later frames must agree with it.
    pub(crate) rate: u32,
}

/// Decodes one frame starting at `r.pos()`: checks the sync code, parses
/// the header, verifies its CRC-8, decodes every subframe, undoes the
/// channel assignment, checks the zero padding and the trailing CRC-16.
///
/// `rate_expected` is the rate previous frames (or STREAMINFO) resolved
/// to; a frame whose coded rate disagrees with it is `Error::BadValue`
/// per RFC 9639's rule that sample rate is uniform across the stream.
/// `info` supplies the frame's coded bps and the STREAMINFO cross-checks.
pub(crate) fn frame(
    r: &mut FlacReader<'_>,
    info: &StreamInfo,
    rate_expected: Option<u32>,
) -> Result<FrameOut> {
    let frame_start = r.pos() / 8;

    // --- header (RFC 9639 section 9.2) ---
    if r.bits(15, "FLAC frame sync")? != 0x7FFC {
        return Err(Error::InvalidMagic {
            what: "FLAC frame sync",
        });
    }
    let _blocking_variable = r.bit("blocking strategy")?;
    let block_code = r.bits(4, "block size code")? as u8;
    let rate_code = r.bits(4, "sample rate code")? as u8;
    let chan_code = r.bits(4, "channel assignment")? as u8;
    let bps_code = r.bits(3, "sample size code")? as u8;
    if r.bit("frame header reserved bit")? != 0 {
        return Err(Error::BadValue("FLAC frame header reserved bit"));
    }

    // The coded frame/sample number is a UTF-8-style integer; the value
    // itself is bookkeeping this decoder does not need.
    let _number = coded_number(r)?;

    // Coded block size: the 8/16-bit extras sit after the coded number,
    // in that order, before the sample-rate extra (RFC 9639 table 13).
    let block_size: u32 = match block_code {
        0 => return Err(Error::BadValue("FLAC reserved block size code")),
        1 => 192,
        2..=5 => 576 << (block_code - 2),
        6 => r.bits(8, "8-bit block size")? as u32 + 1,
        7 => r.bits(16, "16-bit block size")? as u32 + 1,
        _ => 256 << (block_code - 8),
    };

    // Coded sample rate (table 14): code 0 defers to STREAMINFO, codes
    // 12-14 carry the rate in the header, 15 is invalid.
    let rate: u32 = match rate_code {
        0 => info.sample_rate,
        1..=11 => 1_000 * u32::from(rate_code),
        12 => 1_000 * r.bits(8, "8-bit sample rate kHz")? as u32,
        13 => r.bits(16, "16-bit sample rate Hz")? as u32,
        14 => 10 * r.bits(16, "16-bit sample rate daHz")? as u32,
        _ => return Err(Error::BadValue("FLAC invalid sample rate code")),
    };
    if let Some(expected) = rate_expected {
        if rate != expected {
            return Err(Error::BadValue("FLAC frame sample rate"));
        }
    }

    let map = match chan_code {
        0..=7 => ChannelMap::Independent(chan_code + 1),
        8 => ChannelMap::LeftSide,
        9 => ChannelMap::RightSide,
        10 => ChannelMap::MidSide,
        _ => return Err(Error::BadValue("FLAC reserved channel assignment")),
    };
    let nch = map.count();
    if nch != info.channels {
        return Err(Error::BadValue("FLAC frame channel count"));
    }

    // Coded bits per sample (table 15): code 0 defers to STREAMINFO, the
    // rest are literal widths.
    let bps: u8 = match bps_code {
        0 => info.bits_per_sample,
        1 => 8,
        2 => 12,
        3 => return Err(Error::BadValue("FLAC reserved sample size code")),
        4 => 16,
        5 => 20,
        6 => 24,
        _ => 32,
    };
    if bps != info.bits_per_sample {
        return Err(Error::BadValue("FLAC frame bits per sample"));
    }

    // The header is byte-aligned by construction: every field above ends
    // on a boundary. CRC-8 covers the sync byte through the last header
    // byte (RFC 9639 section 9.2.7).
    let header_end = r.pos() / 8;
    let got_crc8 = r.bits(8, "frame header CRC-8")? as u8;
    if crc8(&r.data()[frame_start..header_end]) != got_crc8 {
        return Err(Error::BadValue("FLAC frame header CRC-8"));
    }

    // --- subframes (RFC 9639 section 9.3) ---
    let side = map.side_channel();
    let mut channels: Vec<Vec<i64>> = Vec::with_capacity(usize::from(nch));
    for c in 0..nch {
        let boost = u32::from(side == Some(c));
        channels.push(subframe(r, block_size, u32::from(bps) + boost)?);
    }

    let samples = decorrelate(map, channels)?;
    r.align_zero("FLAC frame padding")?;
    let data_end = r.pos() / 8;
    let got_crc16 = r.bits(16, "FLAC frame CRC-16")? as u16;
    if crc16(&r.data()[frame_start..data_end]) != got_crc16 {
        return Err(Error::BadValue("FLAC frame CRC-16"));
    }
    Ok(FrameOut { samples, rate })
}

/// Reads the UTF-8-style coded frame/sample number (RFC 9639 section
/// 9.2.1): 1-7 bytes, up to 36 significant bits. The encoded value is
/// returned; callers that only need framing discard it.
fn coded_number(r: &mut FlacReader<'_>) -> Result<u64> {
    let b0 = r.bits(8, "coded number")? as u32;
    let (cont, init): (u32, u64) = match b0 {
        0x00..=0x7F => return Ok(u64::from(b0)),
        0xC0..=0xDF => (1, u64::from(b0 & 0x1F)),
        0xE0..=0xEF => (2, u64::from(b0 & 0x0F)),
        0xF0..=0xF7 => (3, u64::from(b0 & 0x07)),
        0xF8..=0xFB => (4, u64::from(b0 & 0x03)),
        0xFC..=0xFD => (5, u64::from(b0 & 0x01)),
        0xFE => (6, 0),
        _ => return Err(Error::BadValue("FLAC coded number lead byte")),
    };
    let mut v = init;
    for _ in 0..cont {
        let b = r.bits(8, "coded number continuation")? as u32;
        if b & 0xC0 != 0x80 {
            return Err(Error::BadValue("FLAC coded number continuation"));
        }
        v = (v << 6) | u64::from(b & 0x3F);
    }
    Ok(v)
}

/// Decodes one subframe of `block_size` samples coded at `sub_bps` bits
/// (already including the stereo side-channel boost).
fn subframe(r: &mut FlacReader<'_>, block_size: u32, sub_bps: u32) -> Result<Vec<i64>> {
    if r.bit("subframe pad bit")? != 0 {
        return Err(Error::BadValue("FLAC subframe pad bit"));
    }
    let ty = r.bits(6, "subframe type")? as u8;
    let wasted_flag = r.bit("wasted bits flag")? != 0;
    let wasted = if wasted_flag {
        // Unary-coded count plus one (RFC 9639 section 9.3.1).
        r.unary("wasted bits")? + 1
    } else {
        0
    };
    if wasted >= sub_bps {
        return Err(Error::BadValue("FLAC wasted bits per sample"));
    }
    let eff = sub_bps - wasted;

    let mut samples = match ty {
        0 => {
            let v = r.signed(eff, "constant subframe")?;
            vec![v; block_size as usize]
        }
        1 => {
            let mut out = Vec::with_capacity(block_size as usize);
            for _ in 0..block_size {
                out.push(r.signed(eff, "verbatim subframe")?);
            }
            out
        }
        8..=12 => fixed(r, block_size, u32::from(ty - 8), eff)?,
        32..=63 => return Err(Error::Unsupported("FLAC LPC subframe")),
        _ => return Err(Error::BadValue("FLAC reserved subframe type")),
    };
    for s in &mut samples {
        *s <<= wasted;
    }
    Ok(samples)
}

/// The four fixed predictors (RFC 9639 section 9.3.5). Samples before the
/// predicted region are the warmup; `predict` reconstructs `s[n]` from
/// `s[n-1..]` and the decoded residual `r[n]`. Arithmetic is `i128`: a
/// corrupt residual may push the running sum past `i64`, and that must
/// surface as a named error, not as wraparound.
fn predict(order: u32, w: &[i64], res: &[i64], out: &mut Vec<i64>) -> Result<()> {
    out.extend_from_slice(w);
    for &e in res {
        let (a, b, c, d) = (
            if order >= 1 {
                i128::from(out[out.len() - 1])
            } else {
                0
            },
            if order >= 2 {
                i128::from(out[out.len() - 2])
            } else {
                0
            },
            if order >= 3 {
                i128::from(out[out.len() - 3])
            } else {
                0
            },
            if order >= 4 {
                i128::from(out[out.len() - 4])
            } else {
                0
            },
        );
        let p = match order {
            0 => 0i128,
            1 => a,
            2 => 2 * a - b,
            3 => 3 * a - 3 * b + c,
            _ => 4 * a - 6 * b + 4 * c - d,
        };
        let v = p + i128::from(e);
        if v > i128::from(i64::MAX) || v < i128::from(i64::MIN) {
            return Err(Error::BadValue("FLAC predicted sample overflow"));
        }
        out.push(v as i64);
    }
    Ok(())
}

/// A fixed-predictor subframe: `order` warmup samples at the coded width,
/// then the Rice-coded residual, then reconstruction (RFC 9639 sections
/// 9.3.4-9.3.5).
fn fixed(r: &mut FlacReader<'_>, block_size: u32, order: u32, eff_bps: u32) -> Result<Vec<i64>> {
    if order > block_size {
        return Err(Error::BadValue("FLAC predictor order exceeds block size"));
    }
    let mut warmup = Vec::with_capacity(order as usize);
    for _ in 0..order {
        warmup.push(r.signed(eff_bps, "fixed-predictor warmup")?);
    }
    let residual = rice(r, block_size, order)?;
    let mut out = Vec::with_capacity(block_size as usize);
    predict(order, &warmup, &residual, &mut out)?;
    Ok(out)
}

/// One Rice residual block (RFC 9639 section 9.3.3): a 2-bit method
/// selecting the parameter width, a 4-bit partition order, then per
/// partition either a Rice parameter or an escape carrying a raw width.
///
/// Zig-zag map: even `u` is `u/2`, odd `u` is `-((u+1)/2)`, so the sign
/// bit sits at the bottom of the coded word.
fn rice(r: &mut FlacReader<'_>, block_size: u32, order: u32) -> Result<Vec<i64>> {
    let method = r.bits(2, "residual coding method")? as u8;
    let (pwidth, escape): (u32, u64) = match method {
        0 => (4, 15),
        1 => (5, 31),
        _ => return Err(Error::BadValue("FLAC reserved residual coding method")),
    };
    let porder = r.bits(4, "Rice partition order")? as u32;
    let partitions = 1u32 << porder;
    if block_size % partitions != 0 {
        return Err(Error::BadValue("FLAC Rice partition does not divide block"));
    }
    let part_len = block_size >> porder;
    if porder > 0 && part_len <= order {
        return Err(Error::BadValue("FLAC Rice partition smaller than order"));
    }

    let mut residual = Vec::with_capacity((block_size - order) as usize);
    for p in 0..partitions {
        // The first partition holds order fewer samples: the warmup took
        // those slots (RFC 9639 section 9.3.3, "blocksize >> order minus
        // predictor order" for partition 0).
        let count = if p == 0 { part_len - order } else { part_len };
        let param = r.bits(pwidth, "Rice parameter")?;
        if param == escape {
            // Escaped partition: a 5-bit raw width, then each residual
            // coded verbatim at that width. Width 0 encodes all zeros.
            let nbits = r.bits(5, "Rice escape width")? as u32;
            for _ in 0..count {
                residual.push(r.signed(nbits, "Rice escape residual")?);
            }
        } else {
            for _ in 0..count {
                let q = r.unary("Rice quotient")?;
                let rem = r.bits(param as u32, "Rice remainder")?;
                // `q` is bounded by the input length, so the shift cannot
                // exceed u64 for any input within limits.
                let u = (u64::from(q) << param) | rem;
                residual.push(((u >> 1) as i64) ^ -((u & 1) as i64));
            }
        }
    }
    debug_assert_eq!(residual.len(), (block_size - order) as usize);
    Ok(residual)
}

/// Undoes the stereo channel assignment into interleaved output. Every
/// map yields `block_size * channels` samples, left-then-right per tick.
fn decorrelate(map: ChannelMap, channels: Vec<Vec<i64>>) -> Result<Vec<i64>> {
    let flat = match map {
        ChannelMap::Independent(_) => interleave(&channels),
        ChannelMap::LeftSide => {
            let l = &channels[0];
            let s = &channels[1];
            let mut left = Vec::with_capacity(l.len());
            let mut right = Vec::with_capacity(l.len());
            for i in 0..l.len() {
                left.push(l[i]);
                right.push(l[i] - s[i]);
            }
            interleave(&[left, right])
        }
        ChannelMap::RightSide => {
            let s = &channels[0];
            let r = &channels[1];
            let mut left = Vec::with_capacity(s.len());
            let mut right = Vec::with_capacity(s.len());
            for i in 0..s.len() {
                right.push(r[i]);
                left.push(r[i] + s[i]);
            }
            interleave(&[left, right])
        }
        ChannelMap::MidSide => {
            let m = &channels[0];
            let s = &channels[1];
            let mut left = Vec::with_capacity(m.len());
            let mut right = Vec::with_capacity(m.len());
            for i in 0..m.len() {
                // `m << 1 | (s & 1)` rebuilds `l + r` exactly, so the
                // halves never lose the low bit (RFC 9639 section 9.2.6).
                let m2 = (m[i] << 1) | (s[i] & 1);
                right.push((m2 - s[i]) >> 1);
                left.push((m2 + s[i]) >> 1);
            }
            interleave(&[left, right])
        }
    };
    Ok(flat)
}

/// Interleaves equal-length channel vectors into `[l0, r0, l1, r1, ...]`.
fn interleave(channels: &[Vec<i64>]) -> Vec<i64> {
    let block = channels.first().map_or(0, Vec::len);
    let mut out = Vec::with_capacity(block * channels.len());
    for i in 0..block {
        for ch in channels {
            out.push(ch[i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Zig-zag coding itself: the residual decoder's sign convention is
    /// the thing most easily flipped, so pin it directly.
    #[test]
    fn zigzag_convention() {
        // (u >> 1) ^ -(u & 1): even non-negative, odd negative.
        let dec = |u: u64| ((u >> 1) as i64) ^ -((u & 1) as i64);
        assert_eq!(dec(0), 0);
        assert_eq!(dec(1), -1);
        assert_eq!(dec(2), 1);
        assert_eq!(dec(3), -2);
        assert_eq!(dec(4), 2);
        assert_eq!(dec(u64::from(u32::MAX)), -2_147_483_648);
    }
}
