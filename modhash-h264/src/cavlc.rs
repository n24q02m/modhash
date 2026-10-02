//! CAVLC residual-block decoding (spec 9.2).
//!
//! One [`decode_block`] call produces the coefficient list of a 4x4
//! block (or the 2x2 chroma-DC block, or the 15-coefficient AC list of
//! an Intra-16x16 / chroma block), following spec 9.2.1 – 9.2.4
//! exactly: coeff_token VLC selection by the predicted neighbour count
//! `nC`, level decoding with the adaptive suffix length, `total_zeros`
//! and `run_before` VLCs, then scatter along the zig-zag scan.
//!
//! The VLC tables live in [`crate::tables`]; decoding is a linear match
//! on (length, bits) pairs, which is slower than a tree but keeps the
//! table verbatim from the spec, eliminating a transcription layer.
//!
//! Every malformed code surfaces as [`Error`]: a coeff_token or
//! total_zeros index past the table, `total_coeff` above `max_coeff`,
//! a level prefix longer than 28, or a truncated suffix all `Err` out
//! — no path panics or wraps silently.

use crate::golomb::Br;
use crate::tables::{
    CHROMA_DC_COEFF_TOKEN_BITS, CHROMA_DC_COEFF_TOKEN_LENS, CHROMA_DC_TOTAL_ZEROS_BITS,
    CHROMA_DC_TOTAL_ZEROS_LENS, COEFF_TOKEN_BITS, COEFF_TOKEN_LENS, RUN_BEFORE_BITS,
    RUN_BEFORE_LENS, TOTAL_ZEROS_BITS, TOTAL_ZEROS_LENS,
};
use modhash_primitives::{Error, Result};

/// Maximum `level_prefix` accepted (beyond 28 the level's magnitude
/// exceeds anything a legal stream needs — longer prefixes are stream
/// corruption, not big coefficients).
const MAX_LEVEL_PREFIX: u32 = 28;

/// One decoded residual block: coefficients in scan order inside the
/// block (`levels[s]` is the coefficient at scan position s; unused
/// entries are zero).
#[derive(Clone, Debug, Default)]
pub(crate) struct Residual {
    /// The `total_coeff` count (non-zero coefficient count), also used
    /// as the `nC` contributor for later neighbours.
    pub total_coeff: u8,
    /// Significant coefficients placed at *scan* positions.
    pub levels: [i32; 16],
}

/// Which VLC-table family a block belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    /// 16-coefficient 4x4 (luma DC+AC of an Intra-4x4 block, or a whole
    /// non-16x16 luma block): coeff_token selected by `nC`, total_zeros
    /// from Table 9-7, zig-zag 4x4.
    LumaOrChromaAc4x4,
    /// 15-coefficient 4x4 AC (Intra-16x16 luma AC, chroma AC): same
    /// VLCs, scan starts at position 1 of the zig-zag.
    Ac15,
    /// 2x2 chroma DC: chroma-DC coeff_token table and Table 9-9(a)
    /// total_zeros; `nC` is not used.
    ChromaDc,
}

impl BlockKind {
    fn max_coeff(self) -> usize {
        match self {
            BlockKind::LumaOrChromaAc4x4 => 16,
            BlockKind::Ac15 => 15,
            BlockKind::ChromaDc => 4,
        }
    }
}

/// Linear VLC match: returns the symbol index whose `(len, bits)` pair
/// is a prefix of the stream.
fn vlc(br: &mut Br<'_>, lens: &[u8], bits: &[u8]) -> Result<usize> {
    let mut max_len = 0usize;
    for &l in lens {
        max_len = max_len.max(l as usize);
    }
    let peek_n = max_len.min(br.remaining()).min(32);
    if peek_n == 0 {
        return Err(Error::truncated("vlc", 1, 0));
    }
    let window = br.peek(peek_n)?;
    for (idx, (&l, &b)) in lens.iter().zip(bits.iter()).enumerate() {
        if l == 0 || (l as usize) > peek_n {
            continue;
        }
        let l = l as usize;
        let code = window >> (peek_n - l);
        if code == u32::from(b) {
            let _ = br.bits(l)?;
            return Ok(idx);
        }
    }
    Err(Error::BadValue("h264 CAVLC: unrecognised VLC prefix"))
}

/// coeff_token VLC for a given `nC` (Table 9-5); `nC < 0` selects the
/// chroma-DC table. Returns `(total_coeff, trailing_ones)`.
fn coeff_token(br: &mut Br<'_>, nc: i32) -> Result<(u8, u8)> {
    if nc < 0 {
        let idx = vlc(br, &CHROMA_DC_COEFF_TOKEN_LENS, &CHROMA_DC_COEFF_TOKEN_BITS)?;
        return Ok(((idx / 4) as u8, (idx % 4) as u8));
    }
    let table = match nc {
        0 | 1 => 0,
        2 | 3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    let idx = vlc(br, &COEFF_TOKEN_LENS[table], &COEFF_TOKEN_BITS[table])?;
    Ok(((idx / 4) as u8, (idx % 4) as u8))
}

/// Leading-zero count consumed as `level_prefix`; prefixes longer than
/// [`MAX_LEVEL_PREFIX`] are stream errors.
fn level_prefix(br: &mut Br<'_>) -> Result<u32> {
    let mut n = 0u32;
    loop {
        if br.remaining() == 0 {
            return Err(Error::truncated("level_prefix", (n + 1) as usize, 0));
        }
        if br.bits(1)? == 1 {
            return Ok(n);
        }
        n += 1;
        if n > MAX_LEVEL_PREFIX {
            return Err(Error::BadValue("h264 CAVLC: level_prefix over 28"));
        }
    }
}

/// Decodes one residual block per spec 9.2. `nc` is the context value
/// (spec 9.2.1); it is ignored for [`BlockKind::ChromaDc`].
pub(crate) fn decode_block(br: &mut Br<'_>, nc: i32, kind: BlockKind) -> Result<Residual> {
    let max_coeff = kind.max_coeff();
    let (total_coeff, trailing_ones) = coeff_token(br, nc)?;
    if total_coeff as usize > max_coeff {
        return Err(Error::BadValue("h264 CAVLC: total_coeff exceeds block size"));
    }
    let mut out = Residual {
        total_coeff,
        ..Residual::default()
    };
    if total_coeff == 0 {
        return Ok(out);
    }
    let total = total_coeff as usize;
    let t1 = trailing_ones as usize;
    if t1 > total {
        return Err(Error::BadValue("h264 CAVLC: trailing_ones exceeds total_coeff"));
    }

    let mut level = [0i32; 16];
    // Trailing-one signs, first bit = sign of the last coefficient.
    for l in level.iter_mut().take(t1) {
        *l = if br.bit()? { -1 } else { 1 };
    }

    if t1 < total {
        // Spec 9.2.2: for each remaining level — prefix, then a suffix
        // of levelSuffixSize bits, the levelCode → signed-level map,
        // then the suffixLength update rule. The first non-trailing
        // level (i == trailing_ones) bumps levelCode by 2 when fewer
        // than 3 trailing ones were seen.
        let mut suffix_length: u32 = u32::from(total > 10 && t1 < 3);
        for (i, l) in level.iter_mut().enumerate().take(total).skip(t1) {
            let prefix = level_prefix(br)? as i64;
            let sl = suffix_length;
            let suffix_size: u32 = if prefix < 14 {
                sl
            } else if prefix == 14 {
                if sl == 0 {
                    4
                } else {
                    sl
                }
            } else {
                // prefix 15 -> 12, prefix >= 16 -> prefix - 3 (the
                // "escape" case); unified as prefix - 3.
                (prefix - 3) as u32
            };
            let mut level_code: i64 = prefix.min(15) << sl;
            if suffix_size > 0 {
                level_code += i64::from(br.bits(suffix_size as usize)?);
            }
            if prefix >= 15 && sl == 0 {
                level_code += 15;
            }
            if prefix >= 16 {
                level_code += (1i64 << (prefix - 3)) - 4096;
            }
            if i == t1 && t1 < 3 {
                level_code += 2;
            }
            let lvl = if level_code % 2 == 0 {
                (level_code + 2) / 2
            } else {
                (-level_code - 1) / 2
            };
            *l = lvl as i32;
            if suffix_length == 0 {
                suffix_length = 1;
            }
            // suffixLimit(suffixLength) = 3 << (suffixLength - 1).
            if l.abs() as i64 > (3i64 << (suffix_length - 1)) && suffix_length < 6 {
                suffix_length += 1;
            }
        }
    }

    // total_zeros / run_before scatter (spec 9.2.3 - 9.2.4).
    let zeros_left = if total == max_coeff {
        0usize
    } else {
        let (lens, bits) = match kind {
            BlockKind::ChromaDc => (
                &CHROMA_DC_TOTAL_ZEROS_LENS[total - 1][..],
                &CHROMA_DC_TOTAL_ZEROS_BITS[total - 1][..],
            ),
            _ => (
                &TOTAL_ZEROS_LENS[total - 1][..],
                &TOTAL_ZEROS_BITS[total - 1][..],
            ),
        };
        let idx = vlc(br, lens, bits)?;
        if idx > max_coeff - total {
            return Err(Error::BadValue("h264 CAVLC: total_zeros out of range"));
        }
        idx
    };

    // Place levels back-to-front over the scan positions.
    let mut scan_pos = zeros_left + total - 1;
    out.levels[scan_pos] = level[0];
    let mut zeros = zeros_left;
    for &lv in level.iter().take(total).skip(1) {
        let run_before = if zeros > 0 {
            let row = (zeros - 1).min(6);
            vlc(br, &RUN_BEFORE_LENS[row], &RUN_BEFORE_BITS[row])?
        } else {
            0
        };
        if run_before > zeros {
            return Err(Error::BadValue("h264 CAVLC: run_before exceeds zeros_left"));
        }
        zeros -= run_before;
        if scan_pos < 1 + run_before {
            return Err(Error::BadValue("h264 CAVLC: coefficient position underflow"));
        }
        scan_pos -= 1 + run_before;
        out.levels[scan_pos] = lv;
    }
    Ok(out)
}

/// Maps a scan position inside a residual block to the raster index
/// used by the rest of the decoder.
///
/// * `LumaOrChromaAc4x4`: 4x4 zig-zag positions 0..15 → raster 0..15.
/// * `Ac15`: scan positions 0..14 map to zig-zag positions 1..15
///   (the DC slot is index 0 and coded elsewhere).
/// * `ChromaDc`: 2x2 raster order (the spec's 2x2 scan).
pub(crate) fn raster_index(kind: BlockKind, scan_pos: usize) -> usize {
    match kind {
        BlockKind::LumaOrChromaAc4x4 => crate::tables::ZIGZAG_4X4[scan_pos] as usize,
        BlockKind::Ac15 => crate::tables::ZIGZAG_4X4[scan_pos + 1] as usize,
        BlockKind::ChromaDc => crate::tables::SCAN_2X2[scan_pos] as usize,
    }
}
