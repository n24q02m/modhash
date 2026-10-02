//! Macroblock model: types, per-MB decode state, and the neighbour
//! geometry every context-derivation step shares.
//!
//! Two coordinate conventions coexist in the spec and both appear here:
//!
//! * **4x4 raster index** `r = x4 + 4*y4` — the order intra-4x4 mode
//!   parsing and reconstruction use;
//! * **8x8-group-major index** `b` (see [`crate::tables::block_index`])
//!   — the order `coded_block_pattern`, CAVLC `nC` derivation, MV
//!   storage and deblocking boundary strengths use. All per-4x4 arrays
//!   in [`MbState`] are group-major.

use crate::tables::block_index;
use modhash_primitives::{Error, Result};

/// Parsed `mb_type` semantics, covering every baseline P- and I-slice
/// code (spec Tables 7-11, 7-13, 7-14).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum MbType {
    /// I_NxN: sixteen 4x4 intra predictions.
    I4x4,
    /// I_16x16 with `intra16x16_pred_mode` (0..3), chroma cbp (0..2) and
    /// luma cbp (0 or 15).
    I16x16 {
        /// `Intra16x16PredMode` 0..=3 (DC, H, V, Plane).
        pred: u8,
        /// `CodedBlockPatternChroma` 0..=2.
        cbp_chroma: u8,
        /// `CodedBlockPatternLuma` 0 or 15.
        cbp_luma: u8,
    },
    /// I_PCM: raw 8-bit samples, no transform.
    IPcm,
    /// P_L0_16x16.
    P16x16,
    /// P_L0_L0_16x8.
    P16x8,
    /// P_L0_L0_8x16.
    P8x16,
    /// P_8x8 (any sub-mb types).
    P8x8,
    /// P_8x8ref0 (sub-mb ref index list implicitly all zero).
    P8x8Ref0,
    /// P_Skip (never parsed as an mb_type; produced by the skip run).
    PSkip,
}

impl MbType {
    /// `true` for any intra mode (4x4, 16x16, PCM).
    pub(crate) fn is_intra(self) -> bool {
        matches!(self, MbType::I4x4 | MbType::I16x16 { .. } | MbType::IPcm)
    }

    /// `true` when the macroblock predicts as sixteen 4x4 blocks.
    pub(crate) fn is_4x4_intra(self) -> bool {
        matches!(self, MbType::I4x4)
    }

    /// Decodes an I-slice `mb_type` code number (spec Table 7-11).
    pub(crate) fn i_slice(code: u32) -> Result<MbType> {
        if code == 0 {
            return Ok(MbType::I4x4);
        }
        if code == 25 {
            return Ok(MbType::IPcm);
        }
        if !(1..=24).contains(&code) {
            return Err(Error::BadValue("I-slice mb_type over 25"));
        }
        let v = code - 1;
        Ok(MbType::I16x16 {
            pred: (v % 4) as u8,
            cbp_chroma: ((v % 12) / 4) as u8,
            cbp_luma: if v < 12 { 0 } else { 15 },
        })
    }

    /// Decodes a P-slice `mb_type` code number (spec Table 7-14):
    /// 0..4 are inter, 5..30 are the I-slice table shifted by 5.
    pub(crate) fn p_slice(code: u32) -> Result<MbType> {
        match code {
            0 => Ok(MbType::P16x16),
            1 => Ok(MbType::P16x8),
            2 => Ok(MbType::P8x16),
            3 => Ok(MbType::P8x8),
            4 => Ok(MbType::P8x8Ref0),
            5..=30 => MbType::i_slice(code - 5),
            _ => Err(Error::BadValue("P-slice mb_type over 30")),
        }
    }
}

/// P_8x8 sub-macroblock types (spec Table 7-17): `code` is the
/// `sub_mb_type` syntax value.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SubMbType {
    /// P_L0_8x8.
    S8x8,
    /// P_L0_8x4.
    S8x4,
    /// P_L0_4x8.
    S4x8,
    /// P_L0_4x4.
    S4x4,
}

impl SubMbType {
    /// Decodes the `sub_mb_type` code (0..=3).
    pub(crate) fn from_code(code: u32) -> Result<SubMbType> {
        match code {
            0 => Ok(SubMbType::S8x8),
            1 => Ok(SubMbType::S8x4),
            2 => Ok(SubMbType::S4x8),
            3 => Ok(SubMbType::S4x4),
            _ => Err(Error::BadValue("sub_mb_type over 3")),
        }
    }

    /// `(w, h)` of each sub-partition in luma samples, and the count.
    pub(crate) fn parts(self) -> ([(u8, u8); 4], usize) {
        match self {
            SubMbType::S8x8 => ([(8, 8), (0, 0), (0, 0), (0, 0)], 1),
            SubMbType::S8x4 => ([(8, 4), (8, 4), (0, 0), (0, 0)], 2),
            SubMbType::S4x8 => ([(4, 8), (4, 8), (0, 0), (0, 0)], 2),
            SubMbType::S4x4 => ([(4, 4); 4], 4),
        }
    }
}

/// Per-macroblock decoded state, shared by prediction, CAVLC `nC` and
/// deblocking. All per-4x4 arrays are 8x8-group-major
/// ([`crate::tables::block_index`]).
#[derive(Clone, Debug)]
pub(crate) struct MbState {
    /// Macroblock type.
    pub mb_type: MbType,
    /// Slice this macroblock belongs to (for cross-slice checks).
    pub slice_id: u32,
    /// `QPy` for this macroblock.
    pub qp_y: u8,
    /// Deblocking idc of the slice that coded this MB (needed when
    /// neighbouring MBs live in different slices).
    pub disable_deblock_idc: u8,
    /// `alpha`/`beta` deblock offsets of the coding slice.
    pub filter_offset_a: i8,
    /// `beta` offset.
    pub filter_offset_b: i8,
    /// `total_coeff` per 4x4 luma block (group-major, 24 entries cover
    /// 16 luma + 8 chroma AC, in the same group-major order for the
    /// chroma 8x8 pair) — used by `nC` prediction and deblocking.
    pub nz: [u8; 24],
    /// Luma motion vectors per 4x4 block (quarter-pel units).
    pub mv: [[i16; 2]; 16],
    /// `ref_idx_l0` per 4x4 block (0xff = not inter-coded).
    pub ref_idx: [u8; 16],
    /// Parsed intra-4x4 modes (raster order), 0..8 or 0xff unset.
    pub i4x4_modes: [u8; 16],
    /// `intra_chroma_pred_mode` 0..=3.
    pub chroma_pred: u8,
}

impl MbState {
    /// Fresh state for an undecoded macroblock.
    pub(crate) fn new() -> MbState {
        MbState {
            mb_type: MbType::PSkip,
            slice_id: u32::MAX,
            qp_y: 26,
            disable_deblock_idc: 0,
            filter_offset_a: 0,
            filter_offset_b: 0,
            nz: [0; 24],
            mv: [[0; 2]; 16],
            ref_idx: [0xff; 16],
            i4x4_modes: [0xff; 16],
            chroma_pred: 0,
        }
    }
}

/// Position of the MB, 4x4-grid coordinates of its top-left sample and
/// neighbour MB indices within a `mb_width` × `mb_height` map.
#[derive(Copy, Clone, Debug)]
pub(crate) struct MbMap {
    /// MB raster index.
    pub idx: usize,
    /// `mb_x`, `mb_y` in macroblocks.
    pub x: usize,
    /// Row.
    pub y: usize,
    /// Picture width in MBs.
    pub width: usize,
}

impl MbMap {
    /// Index of the left neighbour MB (mbAddrA), if any.
    pub(crate) fn mb_a(self) -> Option<usize> {
        (self.x > 0).then_some(self.idx - 1)
    }
    /// Index of the top neighbour MB (mbAddrB), if any.
    pub(crate) fn mb_b(self) -> Option<usize> {
        (self.y > 0).then_some(self.idx - self.width)
    }
    /// Index of the top-right neighbour MB (mbAddrC), if any.
    pub(crate) fn mb_c(self) -> Option<usize> {
        (self.y > 0 && self.x + 1 < self.width).then_some(self.idx - self.width + 1)
    }
    /// Index of the top-left neighbour MB (mbAddrD), if any.
    pub(crate) fn mb_d(self) -> Option<usize> {
        (self.y > 0 && self.x > 0).then_some(self.idx - self.width - 1)
    }
}

/// For the group-major 4x4 block index `blk` of the current MB, returns
/// the left-neighbour block as `(Option<neighbour_mb>, index-in-that-mb)`
/// where a `None` mb means "same macroblock" (group-major index too).
/// This is `h264bsd`'s `N_A_4x4B` table, derived geometrically: the
/// left neighbour of 4x4 block (x4, y4) is (x4-1, y4) when x4 > 0 else
/// (3, y4) of mbAddrA — but expressed in group-major indices.
pub(crate) fn n_a(blk: usize) -> (bool, usize) {
    let (x4, y4) = crate::tables::block_xy(blk);
    if x4 > 0 {
        (false, block_index(x4 - 1, y4))
    } else {
        // In mbA: its right-edge block at the same row. mbA's (3, y4)
        // in ITS group-major order.
        (true, block_index(3, y4))
    }
}

/// Top neighbour of group-major block `blk`: `(true, mbB-index)` or
/// `(false, same-mb index)`. Spec: block containing (xP, yP-1).
pub(crate) fn n_b(blk: usize) -> (bool, usize) {
    let (x4, y4) = crate::tables::block_xy(blk);
    if y4 > 0 {
        (false, block_index(x4, y4 - 1))
    } else {
        (true, block_index(x4, 3))
    }
}

/// Neighbours of the *raster*-index 4x4 block `r` used by intra-4x4
/// prediction (spec 6.4.11.4). Returns
/// `(a, b, c, d)` where each is `(mb, x4, y4)` — `mb` is one of
/// `NeighbourMb`. `None` means unavailable by construction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Nb {
    /// Same macroblock.
    Curr,
    /// mbAddrA (left).
    A,
    /// mbAddrB (above).
    B,
    /// mbAddrC (above-right).
    C,
    /// mbAddrD (above-left).
    D,
    /// Unavailable (out of picture or not yet coded).
    None,
}

/// `(mb, x4, y4)` triple naming a 4x4 block for prediction contexts.
pub(crate) type BlockRef = (Nb, usize, usize);

/// Neighbour 4x4 blocks of raster block `r` inside the MB
/// (spec 6.4.11.4 + 8.3.1.1 availability by construction):
/// `(A, B, C, D)` as luma-block coordinates.
pub(crate) fn intra4x4_neighbours(r: usize) -> [BlockRef; 4] {
    let x = r % 4;
    let y = r / 4;
    let a = if x > 0 {
        (Nb::Curr, x - 1, y)
    } else {
        (Nb::A, 3, y)
    };
    let b = if y > 0 {
        (Nb::Curr, x, y - 1)
    } else {
        (Nb::B, x, 3)
    };
    // C: the block containing the above-right samples (xP+4, yP-1),
    // i.e. 4x4 block (x+1, y-1) of the current MB when both in range.
    let c = if y > 0 {
        if x < 3 {
            (Nb::Curr, x + 1, y - 1)
        } else {
            // (4, y-1) belongs to the right-hand MB, not yet coded.
            (Nb::None, 0, 0)
        }
    } else if x < 3 {
        // y = -1 stays in the MB above (same x span): mbAddrB.
        (Nb::B, x + 1, 3)
    } else {
        // x = 16..19, y = -1: bottom-left of mbAddrC.
        (Nb::C, 0, 3)
    };
    let d = match (x > 0, y > 0) {
        (true, true) => (Nb::Curr, x - 1, y - 1),
        (false, true) => (Nb::A, 3, y - 1),
        (true, false) => (Nb::B, x - 1, 3),
        (false, false) => (Nb::D, 3, 3),
    };
    [a, b, c, d]
}
