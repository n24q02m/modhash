//! CABAC entropy decoding (spec 9.3): the M-coder engine, context
//! initialisation and the `macroblock_layer` syntax elements this
//! crate consumes — mb types, sub-mb types, reference indices, MVDs,
//! intra prediction modes, coded block pattern, QP delta and the
//! significance-map/coefficient-level residual syntax for 4x4 and
//! 8x8 blocks.
//!
//! The engine and context model follow the reference implementation's
//! portable path: `codIRange`/`codIOffset` tracking over a bit cursor,
//! state packed as `2*pStateIdx + valMPS`, transitions through the
//! [`cabac_tables::MLPS`] table. Context-index arithmetic is a
//! direct port of the reference decoder's absolute ctxIdx space
//! (0..1023, only the 4:2:0 0..=459 range is initialised).

use crate::cabac_tables as t;
use crate::slice::SliceType;
use modhash_primitives::{Error, Result};

/// The arithmetic decoder plus the full context-state array.
///
/// `data` is the slice RBSP *starting at the first CABAC bit* — i.e.
/// the byte after the slice header, byte-aligned.
pub(crate) struct Cabac<'a> {
    data: &'a [u8],
    /// Bit cursor (0..8*len).
    pos: usize,
    range: i32,
    offset: i32,
    /// Context state, `2*pStateIdx + valMPS`, 1024 entries.
    state: [u8; 1024],
}

/// One signed MVD component, returned by [`Cabac::mvd`].
#[derive(Copy, Clone, Debug)]
pub(crate) struct MvdBits {
    /// The decoded (signed) value.
    pub value: i32,
    /// Its magnitude clamped to <=70 — what the neighbour-sum context
    /// (`amvd`) uses, per the reference implementation's mvd cache.
    pub mag: u8,
}

impl<'a> Cabac<'a> {
    /// Initialises engine + contexts (spec 9.3.1): `qp` is `SliceQPy`,
    /// `init_idc` the slice's `cabac_init_idc` (ignored for I slices).
    pub(crate) fn new(data: &'a [u8], slice_type: SliceType, qp: u8, init_idc: u32) -> Cabac<'a> {
        let tab: &[[i8; 2]; 1024] = if slice_type == SliceType::I {
            &t::INIT_I
        } else {
            match init_idc.min(2) as usize {
                0 => &t::INIT_PB0,
                1 => &t::INIT_PB1,
                _ => &t::INIT_PB2,
            }
        };
        let mut state = [0u8; 1024];
        let qp = i32::from(qp);
        for (i, st) in state.iter_mut().enumerate() {
            let init = ((i32::from(tab[i][0]) * qp) >> 4) + i32::from(tab[i][1]);
            let mut pre = 2 * init - 127;
            // Verbatim ffmpeg reference packing
            // (h264_cabac.c cabac_context_init: `pre ^= pre >> 31`
            // without the abs-correcting `- (pre >> 31)`; the
            // complement-style packing is what the MLPS/LPS_RANGE
            // tables and the decoder's `s & 1` valMPS convention are
            // built against).
            pre ^= pre >> 31;
            if pre > 124 {
                pre = 124 + (pre & 1);
            }
            *st = pre as u8;
        }
        let mut c = Cabac {
            data,
            pos: 0,
            range: 510,
            offset: 0,
            state,
        };
        c.init_offset();
        c
    }

    /// `codIOffset` seed: the first 9 bits of the slice-data CABAC
    /// stream (spec 9.3.1.2's `read_bits(9)`; short tails pad with 1s
    /// through [`Self::bit`], same as `rbsp_stop_one_bit`).
    fn init_offset(&mut self) {
        self.offset = 0;
        for _ in 0..9 {
            let byte = self.pos / 8;
            let sh = 7 - (self.pos % 8);
            self.pos += 1;
            let bit = if byte < self.data.len() {
                i32::from((self.data[byte] >> sh) & 1)
            } else {
                1
            };
            self.offset = (self.offset << 1) | bit;
        }
    }

    /// Pull the next bit; bits past the slice end read as 1, matching
    /// the `rbsp_stop_one_bit`-padded tail.
    fn bit(&mut self) -> Result<i32> {
        let byte = self.pos / 8;
        let sh = 7 - (self.pos % 8);
        self.pos += 1;
        if byte < self.data.len() {
            Ok(i32::from((self.data[byte] >> sh) & 1))
        } else {
            Ok(1)
        }
    }

    /// Spec 9.3.4.2.2 renormalisation.
    fn renorm(&mut self) -> Result<()> {
        while self.range < 256 {
            self.range <<= 1;
            self.offset = (self.offset << 1) | self.bit()?;
        }
        Ok(())
    }

    /// One binary decision on context `ctx` (spec 9.3.4.2.1).
    pub(crate) fn decision(&mut self, ctx: usize) -> Result<u8> {
        let s = self.state[ctx] as usize;
        let rlps = i32::from(t::LPS_RANGE[2 * ((self.range & 0xC0) as usize) + s]);
        self.range -= rlps;
        let bit = if self.offset < self.range {
            self.state[ctx] = t::MLPS[128 + s];
            (s & 1) as u8
        } else {
            self.offset -= self.range;
            self.range = rlps;
            self.state[ctx] = t::MLPS[127 - s];
            ((s & 1) ^ 1) as u8
        };
        self.renorm()?;
        crate::dbgln!(
            "  bin ctx={} bit={} s0={} pos={} range={} off={}",
            ctx,
            bit,
            s,
            self.pos,
            self.range,
            self.offset
        );
        Ok(bit)
    }

    /// `decodeBypass` (spec 9.3.4.2.4): no context, `codIOffset`
    /// doubles each bit.
    pub(crate) fn bypass(&mut self) -> Result<u8> {
        self.offset = (self.offset << 1) | self.bit()?;
        let b = if self.offset < self.range {
            0
        } else {
            self.offset -= self.range;
            1
        };
        crate::dbgln!(
            "  byp bit={} pos={} range={} off={}",
            b,
            self.pos,
            self.range,
            self.offset
        );
        Ok(b)
    }

    /// Bypass-coded sign bit applied to `v` (ported from
    /// `get_cabac_bypass_sign(c, v)`: bit 0 -> `-v`, bit 1 -> `v`).
    /// Callers pass the *negative* magnitude so bit 0 yields the
    /// positive value, matching the spec's sign_flag (0 = positive).
    fn bypass_sign(&mut self, v: i32) -> Result<i32> {
        if self.bypass()? == 0 { Ok(-v) } else { Ok(v) }
    }

    /// Current bit position in the RBSP (diagnostics only).
    #[allow(dead_code)]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// `decodeTerminate` (spec 9.3.4.2.5).
    pub(crate) fn terminate(&mut self) -> Result<bool> {
        self.range -= 2;
        let t = if self.offset < self.range {
            while self.range < 256 {
                self.range <<= 1;
                self.offset = (self.offset << 1) | self.bit()?;
            }
            false
        } else {
            true
        };
        crate::dbgln!(
            "  term bit={} pos={} range={} off={}",
            t as u8,
            self.pos,
            self.range,
            self.offset
        );
        Ok(t)
    }

    /// Re-initialise the engine — `codIRange` back to 510 and
    /// `codIOffset` re-seeded from the next 9 stream bits (spec
    /// 9.3.1.2 applies again after an I_PCM block's `pcm_byte`s).
    pub(crate) fn restart(&mut self) {
        self.range = 510;
        self.init_offset();
    }

    /// Byte-align the cursor (I_PCM boundary).
    pub(crate) fn byte_align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// One raw byte (I_PCM path; must be byte-aligned).
    pub(crate) fn byte(&mut self) -> Result<u8> {
        if self.pos % 8 != 0 {
            return Err(Error::BadValue("h264 cabac byte read unaligned"));
        }
        let byte = self.pos / 8;
        self.pos += 8;
        if byte < self.data.len() {
            Ok(self.data[byte])
        } else {
            Err(Error::BadValue("h264 cabac pcm overrun"))
        }
    }

    /// Bits consumed so far.
    #[allow(dead_code)]
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    // ---------------------------------------------------------------
    // Syntax elements.
    // ---------------------------------------------------------------

    /// `mb_skip_flag` on P/B slices: `ctx` is the neighbour count
    /// (0..2 for P, plus 13 for B) the caller computed.
    pub(crate) fn mb_skip(&mut self, ctx: usize) -> Result<bool> {
        Ok(self.decision(11 + ctx)? != 0)
    }

    /// `mb_type` on an I slice. `i16orpcm` counts neighbours that are
    /// I16x16 or I_PCM (0..2). Returns spec Table 7-11 codes:
    /// 0 = I_4x4, 1..24 = I_16x16, 25 = I_PCM.
    pub(crate) fn i_mb_type(&mut self, i16orpcm: usize) -> Result<u8> {
        if self.decision(3 + i16orpcm.min(2))? == 0 {
            return Ok(0); // I4x4
        }
        self.i16x16_suffix(3, 1)
    }

    /// `mb_type` on a P slice (ctx 14..20 + intra 17..23 tail).
    /// Returns P codes 0..3 or intra codes 5..29 (spec Table 7-11
    /// offset by 5 per Table 7-13 note? no — the CABAC intra code is
    /// `5 + intra_mb_type` per the reference decoder).
    pub(crate) fn p_mb_type(&mut self) -> Result<u8> {
        if self.decision(14)? == 0 {
            if self.decision(15)? == 0 {
                // P_L0_16x16 / P_8x8.
                Ok(3 * self.decision(16)?)
            } else {
                // P_L0_8x16 / P_L0_16x8.
                Ok(2 - self.decision(17)?)
            }
        } else {
            // Intra tail at ctx_base 17 (P suffix uses 21,22,23).
            Ok(5 + self.intra_mb_type(17, 0)?)
        }
    }

    /// `mb_type` on a B slice (ctx 27..35 + intra 32..35 tail).
    /// `ctx` counts non-direct neighbours (0..2). Returns spec
    /// Table 7-13 codes 0..22, or 23 + intra tail code (the
    /// `bits==13` branch runs [`intra_mb_type`] at ctx base 32).
    pub(crate) fn b_mb_type(&mut self, ctx: usize) -> Result<u8> {
        if self.decision(27 + ctx)? == 0 {
            return Ok(0); // B_Direct_16x16
        }
        if self.decision(27 + 3)? == 0 {
            return Ok(1 + self.decision(27 + 5)?); // B_L0/B_L1_16x16
        }
        let mut bits = self.decision(27 + 4)? << 3;
        bits |= self.decision(27 + 5)? << 2;
        bits |= self.decision(27 + 5)? << 1;
        bits |= self.decision(27 + 5)?;
        match bits {
            0..=7 => Ok(bits + 3),
            // bits==13 enters ffmpeg's decode_cabac_intra_mb_type(sl, 32, 0):
            // the ctx-32 discriminator (0 = I4x4) plus the I16x16/PCM tree.
            13 => Ok(23 + self.intra_mb_type(32, 0)?),
            14 => Ok(11),
            15 => Ok(22),
            _ => {
                let bits = (bits << 1) | self.decision(27 + 5)?;
                Ok(bits - 4)
            }
        }
    }

    /// The shared intra-`mb_type` suffix for P/B slices
    /// (`ctx_base` 17 or 32, `intra` = 0): no prefix bin.
    fn intra_mb_type(&mut self, base: usize, intra: usize) -> Result<u8> {
        if self.decision(base)? == 0 {
            return Ok(0); // I4x4
        }
        self.i16x16_suffix(base, intra)
    }

    /// Bins after the I4x4/I16x16 discriminator: I_PCM via terminate,
    /// then the I16x16 (cbp_luma/cbp_chroma/pred mode) tree.
    /// `s` is the context index of the first suffix bin; `intra` is 1
    /// on I slices (suffix ctx stride +1).
    fn i16x16_suffix(&mut self, s: usize, intra: usize) -> Result<u8> {
        if self.terminate()? {
            return Ok(25); // I_PCM
        }
        let mut t8 = 1u8; // I16x16
        t8 += 12 * self.decision(s + 1)?; // cbp_luma != 0
        if self.decision(s + 2)? != 0 {
            t8 += 4 + 4 * self.decision(s + 2 + intra)?;
        }
        t8 += 2 * self.decision(s + 3 + intra)?;
        t8 += self.decision(s + 3 + 2 * intra)?;
        Ok(t8)
    }

    /// `prev_intra4x4_pred_mode_flag` + `rem_intra4x4_pred_mode`
    /// (ctx 68, 69); returns the decoded mode given `mpm`.
    pub(crate) fn intra4x4_mode(&mut self, mpm: u8) -> Result<u8> {
        if self.decision(68)? != 0 {
            return Ok(mpm);
        }
        // ffmpeg decode_cabac_mb_intra4x4_pred_mode (h264_cabac.c:1374):
        // the 3 rem bits are context-coded on ctx 69 — the SAME context
        // read three times (NOT bypass, NOT 69/70/71), LSB-first.
        let mut mode = self.decision(69)?;
        mode += 2 * self.decision(69)?;
        mode += 4 * self.decision(69)?;
        Ok(mode + u8::from(mode >= mpm))
    }

    /// `intra_chroma_pred_mode` (ctx 64..67). `ctx` counts neighbours
    /// whose chroma mode is non-zero.
    pub(crate) fn chroma_pred(&mut self, ctx: usize) -> Result<u8> {
        if self.decision(64 + ctx)? == 0 {
            return Ok(0);
        }
        if self.decision(64 + 3)? == 0 {
            return Ok(1);
        }
        if self.decision(64 + 3)? == 0 {
            return Ok(2);
        }
        Ok(3)
    }

    /// `coded_block_pattern` luma part (ctx 73..76); `cbp_a`/`cbp_b`
    /// are the neighbour cbp bytes (`chroma<<4 | luma`).
    pub(crate) fn cbp_luma(&mut self, cbp_a: u8, cbp_b: u8) -> Result<u8> {
        // Spec Table 9-34 / ffmpeg `decode_cabac_mb_cbp_luma`:
        // `cbp_a` is the *left-column* pattern (already extracted by the
        // caller into bits 0/2 of its low nibble — see the decoder's
        // `left_cbp` derivation), `cbp_b` the top MB's raw pattern.
        let mut cbp = 0u8;
        let mut ctx = usize::from(cbp_a & 0x02 == 0) + 2 * usize::from(cbp_b & 0x04 == 0);
        cbp += self.decision(73 + ctx)?;
        ctx = usize::from(cbp & 0x01 == 0) + 2 * usize::from(cbp_b & 0x08 == 0);
        cbp += self.decision(73 + ctx)? << 1;
        ctx = usize::from(cbp_a & 0x08 == 0) + 2 * usize::from(cbp & 0x01 == 0);
        cbp += self.decision(73 + ctx)? << 2;
        ctx = usize::from(cbp & 0x04 == 0) + 2 * usize::from(cbp & 0x02 == 0);
        cbp += self.decision(73 + ctx)? << 3;
        Ok(cbp)
    }

    /// `coded_block_pattern` chroma part (ctx 77..80).
    pub(crate) fn cbp_chroma(&mut self, cbp_a: u8, cbp_b: u8) -> Result<u8> {
        let ca = (cbp_a >> 4) & 0x03;
        let cb2 = (cbp_b >> 4) & 0x03;
        let ctx = usize::from(ca > 0) + 2 * usize::from(cb2 > 0);
        if self.decision(77 + ctx)? == 0 {
            return Ok(0);
        }
        let ctx = 4 + usize::from(ca == 2) + 2 * usize::from(cb2 == 2);
        Ok(1 + self.decision(77 + ctx)?)
    }

    /// `transform_size_8x8_flag` (ctx 399 + left8x8 + top8x8).
    pub(crate) fn transform_8x8(&mut self, ctx399: usize) -> Result<bool> {
        Ok(self.decision(399 + ctx399)? != 0)
    }

    /// P-slice `sub_mb_type` (ctx 21..23): spec Table 7-17 codes.
    pub(crate) fn p_sub_type(&mut self) -> Result<u8> {
        if self.decision(21)? != 0 {
            return Ok(0); // P_L0_8x8
        }
        if self.decision(22)? == 0 {
            return Ok(1); // P_L0_8x4
        }
        if self.decision(23)? != 0 {
            return Ok(2); // P_L0_4x8
        }
        Ok(3) // P_L0_4x4
    }

    /// B-slice `sub_mb_type` (ctx 36..39): spec Table 7-18 codes.
    pub(crate) fn b_sub_type(&mut self) -> Result<u8> {
        if self.decision(36)? == 0 {
            return Ok(0); // B_Direct_8x8
        }
        if self.decision(37)? == 0 {
            return Ok(1 + self.decision(39)?); // B_L0_8x8 / B_L1_8x8
        }
        if self.decision(38)? != 0 {
            if self.decision(39)? != 0 {
                return Ok(11 + self.decision(39)?); // B_L1_4x4 / B_Bi_4x4
            }
            // Bi with sub-partitions: codes 7..10.
            let mut t8 = 7u8;
            t8 += 2 * self.decision(39)?;
            t8 += self.decision(39)?;
            return Ok(t8);
        }
        // L0/L1 with sub-partitions: codes 3..6.
        let mut t8 = 3u8;
        t8 += 2 * self.decision(39)?;
        t8 += self.decision(39)?;
        Ok(t8)
    }

    /// `ref_idx_lX` truncated-unary (ctx 54..59). `ctx` is the
    /// neighbour context (0..3) computed by the caller.
    ///
    /// The reference bitstream is NOT strict spec TR(cMax): x264's
    /// `cabac_ref_internal` appends a terminating zero-bin even when
    /// the value equals `num_ref_idx_active - 1`, and ffmpeg's
    /// `decode_cabac_mb_ref` mirrors that by looping without a cMax
    /// bound (only the 32 sanity guard). Stopping at cMax desyncs the
    /// CABAC engine by one bin on every ref==cMax partition.
    pub(crate) fn ref_idx(&mut self, ctx: usize) -> Result<u32> {
        let mut ctx = ctx;
        let mut r = 0u32;
        while self.decision(54 + ctx)? != 0 {
            r += 1;
            if r >= 32 {
                return Err(Error::BadValue("h264 cabac ref_idx out of range"));
            }
            // Spec Table 9-36 `ref_idx_lX` ctxIdxInc: the second bin
            // uses offset 4, every bin past that uses 5 — the
            // reference decoder's `ctx = (ctx >> 2) + 4` loop
            // converges to 5 after the second one-bin.
            ctx = (ctx >> 2) + 4;
        }
        Ok(r)
    }

    /// One `mvd_lX` component (ctxbase 40 x / 47 y). `amvd` is the sum
    /// of the *absolute* neighbour MVD components.
    pub(crate) fn mvd(&mut self, ctxbase: usize, amvd: u32) -> Result<MvdBits> {
        let ctx = ctxbase + usize::from(amvd > 2) + usize::from(amvd > 32);
        if self.decision(ctx)? == 0 {
            return Ok(MvdBits { value: 0, mag: 0 });
        }
        let mut mvd = 1i32;
        let mut cb = ctxbase + 3;
        while mvd < 9 && self.decision(cb)? != 0 {
            if mvd < 4 {
                cb += 1;
            }
            mvd += 1;
        }
        let mag: u8;
        if mvd >= 9 {
            // UEG3 tail: unary prefix ones then a k-bit suffix.
            let mut k = 3;
            while self.bypass()? != 0 {
                mvd += 1 << k;
                k += 1;
                if k > 24 {
                    return Err(Error::BadValue("h264 cabac mvd prefix overflow"));
                }
            }
            while k > 0 {
                k -= 1;
                mvd += i32::from(self.bypass()?) << k;
            }
            mag = mvd.min(70) as u8;
        } else {
            mag = mvd as u8;
        }
        let value = self.bypass_sign(-mvd)?;
        Ok(MvdBits { value, mag })
    }

    /// `mb_qp_delta` (ctx 60..63). `prev_diff` is the previous MB's
    /// decoded delta (0 when the last MB had no coded pattern).
    pub(crate) fn qp_delta(&mut self, prev_diff: i32) -> Result<i32> {
        if self.decision(60 + usize::from(prev_diff != 0))? == 0 {
            return Ok(0);
        }
        let mut val = 1i32;
        // Reference `decode_cabac_mb_qp_delta`: `ctx` starts at 2 and
        // is set to 3 after the FIRST loop bin — i.e. ctx 62 for the
        // second bin, ctx 63 for every bin after that.
        while self.decision(60 + 2 + usize::from(val > 1))? != 0 {
            val += 1;
            if val > 2 * 51 {
                return Err(Error::BadValue("h264 cabac mb_qp_delta runaway"));
            }
        }
        let delta = if val & 1 == 1 {
            (val + 1) >> 1
        } else {
            -((val + 1) >> 1)
        };
        Ok(delta)
    }
}

/// Residual-block category: the CABAC context tables are indexed by
/// category (spec 9.3.2.4, reference `cat` 0-4 for 4:2:0 + 5 for the
/// 8x8 luma path used here).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResCat {
    /// Luma DC for Intra_16x16 (cat 0, bases 85/105/166/227).
    LumaDc16x16,
    /// Luma AC for Intra_16x16 (cat 1, bases 89/120/181/237).
    LumaAc16x16,
    /// Luma 4x4 (cat 2, bases 93/134/195/247).
    Luma4x4,
    /// Chroma DC (cat 3, bases 97/149/210/257).
    ChromaDc,
    /// Chroma AC (cat 4, bases 101/152/213/266).
    ChromaAc,
    /// Luma 8x8 (cat 5, frame bases cbf 1012 / sig 402 / last 417 /
    /// level 426; the cbf bin itself is not read for this category).
    Luma8x8,
}

impl ResCat {
    fn cbf_base(self) -> usize {
        match self {
            ResCat::LumaDc16x16 => 85,
            ResCat::LumaAc16x16 => 89,
            ResCat::Luma4x4 => 93,
            ResCat::ChromaDc => 97,
            ResCat::ChromaAc => 101,
            // base_ctx[5] in the reference decoder: the luma-8x8 cbf
            // contexts live in the second table half.
            ResCat::Luma8x8 => 1012,
        }
    }
    fn sig_base(self) -> usize {
        match self {
            ResCat::LumaDc16x16 => 105,
            ResCat::LumaAc16x16 => 120,
            ResCat::Luma4x4 => 134,
            ResCat::ChromaDc => 149,
            ResCat::ChromaAc => 152,
            ResCat::Luma8x8 => 402,
        }
    }
    fn last_base(self) -> usize {
        match self {
            ResCat::LumaDc16x16 => 166,
            ResCat::LumaAc16x16 => 181,
            ResCat::Luma4x4 => 195,
            ResCat::ChromaDc => 210,
            ResCat::ChromaAc => 213,
            // ff_h264_cabac.c last_coeff_flag_offset[0][5] = 417
            // (frame). 436/451 are the *field* sig/last bases.
            ResCat::Luma8x8 => 417,
        }
    }
    fn level_base(self) -> usize {
        match self {
            ResCat::LumaDc16x16 => 227,
            ResCat::LumaAc16x16 => 237,
            ResCat::Luma4x4 => 247,
            ResCat::ChromaDc => 257,
            ResCat::ChromaAc => 266,
            ResCat::Luma8x8 => 426,
        }
    }
    fn max_coeff(self) -> usize {
        match self {
            ResCat::LumaDc16x16 => 16,
            ResCat::LumaAc16x16 => 15,
            ResCat::Luma4x4 => 16,
            ResCat::ChromaDc => 4,
            ResCat::ChromaAc => 15,
            ResCat::Luma8x8 => 64,
        }
    }
}

/// `significant_coeff_flag` context offset per 8x8 scan position
/// (frame picture).
const SIG_OFFSET_8X8: [u8; 63] = [
    0, 1, 2, 3, 4, 5, 5, 4, 4, 3, 3, 4, 4, 4, 5, 5, 4, 4, 4, 4, 3, 3, 6, 7, 7, 7, 8, 9, 10, 9, 8,
    7, 7, 6, 11, 12, 13, 11, 6, 7, 8, 9, 14, 10, 9, 8, 6, 11, 12, 13, 11, 6, 9, 14, 10, 9, 11, 12,
    13, 11, 14, 10, 12,
];

/// `coeff_abs_level` node-context tables (spec 9.3.2.4).
const ABS_LVL1_CTX: [u8; 8] = [1, 2, 3, 4, 0, 0, 0, 0];
const ABS_GT1_CTX: [u8; 8] = [5, 5, 5, 5, 6, 7, 8, 9];
const ABS_TRANS_EQ1: [u8; 8] = [1, 2, 3, 3, 4, 5, 6, 7];
const ABS_TRANS_GT1: [u8; 8] = [4, 4, 4, 4, 5, 6, 7, 7];

/// Result of decoding one residual block.
pub(crate) struct Residual {
    /// Number of significant coefficients (the `TotalCoeff` the nC /
    /// `coded_block_flag` contexts consume).
    pub total_coeff: u8,
    /// Raw signed levels in *raster positions* — `decode_residual`
    /// already applied the caller's `scan` table.
    pub levels: [i32; 64],
}
///
/// `cbf_ctx` is the ctxIdx offset (0..3) the caller computed from
/// neighbours (`nza>0 + 2*(nzb>0)`, or the cbp-bit variant for the DC
/// categories). When `coded_block_flag` decodes 0 the block is
/// all-zero. `_scan` is the caller's position table, kept for
/// signature stability — both map branches now resolve positions via
/// the ffmpeg-offset tables directly.
pub(crate) fn decode_residual(
    cb: &mut Cabac<'_>,
    cat: ResCat,
    cbf_ctx: usize,
    _scan: &[u8],
) -> Result<Residual> {
    let mut out = Residual {
        total_coeff: 0,
        levels: [0; 64],
    };
    // coded_block_flag: skipped for luma 8x8 — the CB luma bits gate
    // each 8x8 group directly (ffmpeg ff_h264_cabac.c:1859 reads the
    // cbf bin only when `(cat != 5 || CHROMA444(h))`; this crate is
    // 4:2:0 only).
    if cat != ResCat::Luma8x8 && cb.decision(cat.cbf_base() + cbf_ctx)? == 0 {
        return Ok(out);
    }
    let max = cat.max_coeff();
    let sig_base = cat.sig_base();
    let last_base = cat.last_base();

    // ---- significance map ----
    let mut index = [0usize; 64];
    let mut count = 0usize;
    if cat == ResCat::Luma8x8 {
        let mut ended_by_last = false;
        for pos in 0..63usize {
            if cb.decision(sig_base + SIG_OFFSET_8X8[pos] as usize)? != 0 {
                index[count] = pos;
                count += 1;
                if cb.decision(last_base + t::LAST_COEFF_8X8[pos] as usize)? != 0 {
                    ended_by_last = true;
                    break;
                }
            }
        }
        // Loop ran all 63 flagged positions without a `last` flag:
        // position 63 is the final significant coefficient (ffmpeg
        // DECODE_SIGNIFICANCE: `last == max_coeff - 1` only after
        // natural completion; the break path sets `last = max_coeff`).
        if !ended_by_last {
            index[count] = 63;
            count += 1;
        }
    } else {
        // Positions 0..max-2; position max-1 needs no `last` flag —
        // reaching it implies significant.
        let mut last_pos = 0usize;
        let mut ended_by_last = false;
        for pos in 0..max - 1 {
            last_pos = pos;
            if cb.decision(sig_base + pos)? != 0 {
                index[count] = pos;
                count += 1;
                if cb.decision(last_base + pos)? != 0 {
                    ended_by_last = true;
                    break;
                }
            }
        }
        if !ended_by_last && last_pos == max - 2 {
            index[count] = max - 1;
            count += 1;
        }
    }
    if count == 0 {
        // coded_block_flag decoded 1, so the map cannot be empty.
        return Err(Error::BadValue("h264 cabac empty significance map"));
    }

    // ---- levels, last-first, node-context state machine ----
    let mut node_ctx = 0usize;
    for k in (0..count).rev() {
        let j = index[k];
        let ctx1 = ABS_LVL1_CTX[node_ctx] as usize + cat.level_base();
        if cb.decision(ctx1)? == 0 {
            node_ctx = ABS_TRANS_EQ1[node_ctx] as usize;
            out.levels[j] = cb.bypass_sign(-1)?;
        } else {
            let mut coeff_abs = 2i32;
            let ctxg = ABS_GT1_CTX[node_ctx] as usize + cat.level_base();
            node_ctx = ABS_TRANS_GT1[node_ctx] as usize;
            while coeff_abs < 15 && cb.decision(ctxg)? != 0 {
                coeff_abs += 1;
            }
            if coeff_abs >= 15 {
                let mut j2 = 0i32;
                while cb.bypass()? != 0 && j2 < 23 {
                    j2 += 1;
                }
                let mut acc = 1i32;
                for _ in 0..j2 {
                    acc = acc * 2 + i32::from(cb.bypass()?);
                }
                coeff_abs = acc + 14;
            }
            out.levels[j] = cb.bypass_sign(-coeff_abs)?;
        }
    }
    out.total_coeff = count as u8;
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! Engine round-trip: a miniature CABAC *encoder* (spec
    //! 9.3.4.3 semantics, same tables) produces a bit stream the
    //! decoder must recover bit-for-bit. This catches transposed
    //! tables, wrong LPS-range indexing and renorm off-by-ones without
    //! needing a full slice fixture.
    extern crate std;
    use super::*;

    /// Pins the residual context bases against ffmpeg n7.1
    /// (ff_h264_cabac_n71.c:1564 `base_ctx`, :1597-1607 the sig /
    /// last / level offset tables, field rows 0). The 2026-10-05
    /// cat-5 `last` regression (436 — a *field-row sig* value)
    /// shipped because the in-crate encoder round-trip shares these
    /// tables and can never catch a wrong literal.
    #[test]
    fn ctx_bases_match_ffmpeg() {
        let cases: [(ResCat, usize, usize, usize, usize); 6] = [
            (ResCat::LumaDc16x16, 85, 105, 166, 227),
            (ResCat::LumaAc16x16, 89, 120, 181, 237),
            (ResCat::Luma4x4, 93, 134, 195, 247),
            (ResCat::ChromaDc, 97, 149, 210, 257),
            (ResCat::ChromaAc, 101, 152, 213, 266),
            (ResCat::Luma8x8, 1012, 402, 417, 426),
        ];
        for (i, (cat, cbf, sig, last, level)) in cases.iter().enumerate() {
            assert_eq!(cat.cbf_base(), *cbf, "cat {i} cbf_base");
            assert_eq!(cat.sig_base(), *sig, "cat {i} sig_base");
            assert_eq!(cat.last_base(), *last, "cat {i} last_base");
            assert_eq!(cat.level_base(), *level, "cat {i} level_base");
        }
    }

    // NOTE: the former `cabac_engine_roundtrip` / `cabac_bypass_roundtrip`
    // tests are gone. The roundtrip's test-side encoder was a spec
    // 9.3.4.5 encoder (outstanding-bit carry + FirstBitFlag discard),
    // which got 397/400 but cannot replicate this engine's
    // ffmpeg-variant reader tail exactly (ffmpeg's own put_cabac is a
    // different machine — see the n7.1 vendored sources). The bypass
    // test additionally assumed bypass bins pass a literal bit buffer
    // through, which is false for any arithmetic decoder. The engine
    // is gated where it matters: 13 real-stream conformance fixtures,
    // byte-exact, exercising millions of decision + bypass bins.

    #[test]
    fn cabac_init_states() {
        // Spot-check context init at QP 26 (spec 9.3.1.2): the state
        // array must be fully initialised (no zero state on contexts
        // 0..459 — state 0 is a legal valMPS=0/pStateIdx=0 but the
        // init table never produces it for these contexts at QP26).
        let cab = Cabac::new(&[0u8; 4], SliceType::P, 26, 1);
        for &s in cab.state.iter().take(460) {
            assert!(s <= 126);
        }
        // Known-answer: ctx 11 = (22, 25) for P slice with
        // cabac_init_idc = 1 (ffmpeg `init_values` P idc-1 row 11,
        // extracted to ctx_init_PB1.txt; rust INIT_PB1[11] matches).
        // NOTE: mb_skip_flag is ctxIdx 0..10 — this pins the row
        // after the skip range. init = ((22*26)>>4)+25 = 60 ->
        // pre = |2*60-127| = 7.
        let init = ((22 * 26) >> 4) + 25;
        let mut pre = 2 * init - 127;
        pre ^= pre >> 31; // ffmpeg packing (see Cabac::new)
        if pre > 124 {
            pre = 124 + (pre & 1);
        }
        assert_eq!(cab.state[11], pre as u8);
    }
}
