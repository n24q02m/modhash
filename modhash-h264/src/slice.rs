//! Slice header parsing (spec 7.3.3) including reference-picture list
//! reordering, weighted-prediction tables, and reference-picture
//! marking (MMCO) syntax.

use crate::golomb::Br;
use crate::pps::Pps;
use crate::sps::Sps;
use modhash_primitives::{Error, Result};

/// Slice kinds this crate decodes (spec Table 7-6).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SliceType {
    /// P slice (codes 0 and 5).
    P,
    /// I slice (codes 2 and 7) — and the "all intra" codes collapse to I.
    I,
    /// B slice (codes 1 and 6).
    B,
}

/// One `dec_ref_pic_marking` adaptive command (spec 7.4.3.3).
#[derive(Copy, Clone, Debug)]
pub(crate) struct Mmco {
    /// `memory_management_control_operation` (1..=6).
    pub op: u8,
    /// `difference_of_pic_nums_minus1` (ops 1, 3).
    pub difference_of_pic_nums: u32,
    /// `long_term_pic_num` (ops 2, 3, 6).
    pub long_term_pic_num: u32,
    /// `long_term_frame_idx` (ops 4, 6).
    pub long_term_frame_idx: u32,
    /// `max_long_term_frame_idx_plus1` (op 4).
    pub max_long_term_frame_idx: u32,
}

/// Weighted-prediction table entry for one reference index
/// (spec 7.4.3.2): luma + both chroma weights and offsets.
#[derive(Copy, Clone, Debug)]
pub(crate) struct WpEntry {
    /// `luma_weight_l0` and its offset.
    pub luma: (i32, i32),
    /// `chroma_weight_l0`/`chroma_offset_l0` for Cb and Cr.
    pub chroma: [(i32, i32); 2],
    /// `luma_weight_l0_flag`.
    pub luma_flag: bool,
    /// `chroma_weight_l0_flag`.
    pub chroma_flag: bool,
}

/// Parsed slice header fields this decoder consumes.
#[derive(Clone, Debug)]
pub(crate) struct SliceHeader {
    /// `first_mb_in_slice`.
    pub first_mb: u32,
    /// `slice_type` collapsed to P or I.
    pub slice_type: SliceType,
    /// `pic_parameter_set_id`.
    pub pps_id: u32,
    /// `frame_num`.
    pub frame_num: u32,
    /// `idr_pic_id` (IDR slices only); multi-slice IDR pictures must
    /// agree — compared by the decoder when a second IDR slice arrives
    /// for the same picture.
    pub idr_pic_id: u32,
    /// `pic_order_cnt_lsb` (type 0). Baseline streams never reorder —
    /// output order is decode order — so POC is parsed but unread.
    #[allow(dead_code)]
    pub pic_order_cnt_lsb: u32,
    /// `delta_pic_order_cnt_bottom` (type 0 + bottom_field flag).
    #[allow(dead_code)]
    pub delta_poc_bottom: i32,
    /// `delta_pic_order_cnt[0]` (type 1, non-IDR, not always-zero).
    #[allow(dead_code)]
    pub delta_poc0: i32,
    /// `delta_pic_order_cnt[1]`.
    #[allow(dead_code)]
    pub delta_poc1: i32,
    /// `num_ref_idx_active_override_flag`. Folded into the
    /// `num_ref_idx_*_active` fields at parse time.
    #[allow(dead_code)]
    pub num_ref_override: bool,
    /// Effective `num_ref_idx_l0_active` (post-override).
    pub num_ref_idx_l0_active: u32,
    /// Effective `num_ref_idx_l1_active` (B slices; post-override).
    pub num_ref_idx_l1_active: u32,
    /// `ref_pic_list_reordering` commands for L0: `(idc, value)`.
    pub reorder_l0: alloc::vec::Vec<(u32, u32)>,
    /// Reordering commands for L1 (B slices).
    pub reorder_l1: alloc::vec::Vec<(u32, u32)>,
    /// Weighted-prediction table for L0 (`weighted_pred_flag` P or
    /// `weighted_bipred_idc == 1` B).
    pub wp_l0: Option<alloc::vec::Vec<WpEntry>>,
    /// Weighted-prediction table for L1 (explicit B weights only).
    pub wp_l1: Option<alloc::vec::Vec<WpEntry>>,
    /// `log2` denominators for the weighted table.
    pub wp_denom: (u32, u32),
    /// `direct_spatial_mv_pred_flag` (B slices): spatial vs temporal
    /// direct prediction for direct-mode macroblocks.
    pub direct_spatial: bool,
    /// `cabac_init_idc` (CABAC slices, non-I): picks one of the three
    /// P/B context-init table rows.
    pub cabac_init_idc: u32,
    /// IDR marking: `no_output_of_prior_pics_flag`, `long_term_reference_flag`.
    pub idr_marking: (bool, bool),
    /// Non-IDR marking: `adaptive_ref_pic_marking_mode_flag` + MMCO list.
    pub adaptive_marking: bool,
    /// The MMCO command list.
    pub mmco: alloc::vec::Vec<Mmco>,
    /// `slice_qp_delta` (added to `pps.pic_init_qp`).
    pub slice_qp_delta: i32,
    /// `disable_deblocking_filter_idc` (0 = filter everywhere).
    pub disable_deblock_idc: u8,
    /// `slice_alpha_c0_offset_div2` * 2.
    pub offset_a: i8,
    /// `slice_beta_offset_div2` * 2.
    pub offset_b: i8,
}

/// Parses the slice header from `br`, returning the header and leaving
/// `br` at the first `slice_data` bit. `idr` distinguishes IDR NALs.
pub(crate) fn parse_header(
    br: &mut Br<'_>,
    idr: bool,
    nal_ref_idc: u8,
    sps: &Sps,
    pps: &Pps,
) -> Result<SliceHeader> {
    let first_mb = br.ue()?;
    if first_mb >= sps.width_mbs * sps.height_mbs {
        return Err(Error::BadValue("first_mb_in_slice past picture"));
    }
    let st = br.ue()?;
    let slice_type = match st {
        0 | 5 => SliceType::P,
        2 | 7 => SliceType::I,
        1 | 6 => SliceType::B,
        3 | 8 => return Err(Error::Unsupported("h264 SP slice")),
        4 | 9 => return Err(Error::Unsupported("h264 SI slice")),
        _ => return Err(Error::BadValue("slice_type over 9")),
    };
    let pps_id = br.ue()?;
    if pps_id != pps.id {
        return Err(Error::BadValue("slice references a different PPS"));
    }
    let frame_num = br.bits(sps.log2_max_frame_num as usize)?;
    // frame_mbs_only => no field_pic_flag / bottom_field_flag.
    let mut idr_pic_id = 0;
    if idr {
        idr_pic_id = br.ue()?;
        if idr_pic_id > u16::MAX as u32 {
            return Err(Error::BadValue("idr_pic_id over 65535"));
        }
    }
    let mut pic_order_cnt_lsb = 0;
    let mut delta_poc_bottom = 0;
    let mut delta_poc0 = 0;
    let mut delta_poc1 = 0;
    match sps.pic_order_cnt_type {
        0 => {
            pic_order_cnt_lsb = br.bits(sps.log2_max_poc_lsb as usize)?;
            if pps.bottom_field_pic_order {
                delta_poc_bottom = br.se()?;
            }
        }
        1 if !sps.delta_pic_order_always_zero => {
            delta_poc0 = br.se()?;
            if pps.bottom_field_pic_order {
                delta_poc1 = br.se()?;
            }
        }
        _ => {}
    }
    // redundant_pic_cnt is gated by the PPS flag we already refused.
    let mut direct_spatial = false;
    if slice_type == SliceType::B {
        direct_spatial = br.bit()?;
    }
    let mut num_ref_override = false;
    let mut num_ref_idx_l0_active = pps.num_ref_idx_l0_active;
    let mut num_ref_idx_l1_active = pps.num_ref_idx_l1_active;
    if matches!(slice_type, SliceType::P | SliceType::B) {
        num_ref_override = br.bit()?;
        if num_ref_override {
            num_ref_idx_l0_active = br.ue()? + 1;
            if slice_type == SliceType::B {
                num_ref_idx_l1_active = br.ue()? + 1;
            }
            if num_ref_idx_l0_active > 32 || num_ref_idx_l1_active > 32 {
                return Err(Error::BadValue("num_ref_idx_active over 32"));
            }
        }
    }
    // ref_pic_list_reordering (spec 7.4.3.1): L0 on P and B, L1 on B.
    let mut reorder_l0 = alloc::vec::Vec::new();
    let mut reorder_l1 = alloc::vec::Vec::new();
    let mut read_reorder = |list: &mut alloc::vec::Vec<(u32, u32)>, cap: u32| -> Result<()> {
        loop {
            let idc = br.ue()?;
            if idc > 3 {
                return Err(Error::BadValue("reordering_of_pic_nums_idc over 3"));
            }
            if idc == 3 {
                break;
            }
            // idc 0/1 take abs_diff_pic_num_minus1, idc 2 takes
            // long_term_pic_num — all ue(v) in the stream.
            let v = br.ue()?;
            if list.len() >= cap as usize + 2 {
                return Err(Error::BadValue("ref_pic_list_reordering too long"));
            }
            list.push((idc, v));
        }
        Ok(())
    };
    if matches!(slice_type, SliceType::P | SliceType::B) && br.bit()? {
        read_reorder(&mut reorder_l0, num_ref_idx_l0_active)?;
    }
    if slice_type == SliceType::B && br.bit()? {
        read_reorder(&mut reorder_l1, num_ref_idx_l1_active)?;
    }
    // pred_weight_table (spec 7.4.3.2): P under `weighted_pred_flag`,
    // B under `weighted_bipred_idc == 1` (explicit weights). idc 2 is
    // implicit weighting — no table follows the reorder section then.
    let mut wp_l0 = None;
    let mut wp_l1 = None;
    let mut wp_denom = (0u32, 0u32);
    let explicit_wp = (slice_type == SliceType::P && pps.weighted_pred)
        || (slice_type == SliceType::B && pps.weighted_bipred_idc == 1);
    if explicit_wp {
        let luma_denom = br.ue()?;
        let chroma_denom = br.ue()?;
        if luma_denom > 7 || chroma_denom > 7 {
            return Err(Error::BadValue("weight denom over 7"));
        }
        wp_denom = (luma_denom, chroma_denom);
        // One list's table per spec 7.4.3.2 order — luma flags, luma
        // weights, chroma flags, chroma weights — then L1 repeats it.
        let mut read_list = |count: u32| -> Result<alloc::vec::Vec<WpEntry>> {
            let mut luma_flags = alloc::vec::Vec::with_capacity(count as usize);
            for _ in 0..count {
                luma_flags.push(br.bit()?);
            }
            let mut weights = alloc::vec::Vec::with_capacity(count as usize);
            for &f in luma_flags.iter() {
                if f {
                    let w = br.se()?;
                    let o = br.se()?;
                    if !(-128..=127).contains(&w) || !(-128..=127).contains(&o) {
                        return Err(Error::BadValue("luma weight/offset out of range"));
                    }
                    weights.push((w, o));
                } else {
                    weights.push((1i32 << luma_denom, 0));
                }
            }
            let mut chroma_flags = alloc::vec::Vec::with_capacity(count as usize);
            for _ in 0..count {
                chroma_flags.push(br.bit()?);
            }
            let mut chroma_w = alloc::vec::Vec::with_capacity(count as usize);
            for &f in chroma_flags.iter() {
                if f {
                    let mut c = [(0i32, 0i32); 2];
                    for p in c.iter_mut() {
                        *p = (br.se()?, br.se()?);
                        if !(-128..=127).contains(&p.0) || !(-128..=127).contains(&p.1) {
                            return Err(Error::BadValue(
                                "chroma weight/offset out of range",
                            ));
                        }
                    }
                    chroma_w.push(c);
                } else {
                    chroma_w.push([(1i32 << chroma_denom, 0); 2]);
                }
            }
            let mut table = alloc::vec::Vec::with_capacity(count as usize);
            for i in 0..count as usize {
                table.push(WpEntry {
                    luma: weights[i],
                    chroma: chroma_w[i],
                    luma_flag: luma_flags[i],
                    chroma_flag: chroma_flags[i],
                });
            }
            Ok(table)
        };
        wp_l0 = Some(read_list(num_ref_idx_l0_active)?);
        if slice_type == SliceType::B {
            wp_l1 = Some(read_list(num_ref_idx_l1_active)?);
        }
    }

    // dec_ref_pic_marking (spec 7.3.3.3).
    let mut idr_marking = (false, false);
    let mut adaptive_marking = false;
    let mut mmco = alloc::vec::Vec::new();
    if nal_ref_idc != 0 {
        if idr {
            idr_marking = (br.bit()?, br.bit()?);
            if idr_marking.1 {
                return Err(Error::Unsupported(
                    "h264 IDR long-term marking (long_term_reference_flag)",
                ));
            }
        } else {
            adaptive_marking = br.bit()?;
            if adaptive_marking {
                loop {
                    let op = br.ue()?;
                    if op == 0 {
                        break;
                    }
                    if op > 6 {
                        return Err(Error::BadValue("mmco over 6"));
                    }
                    let mut m = Mmco {
                        op: op as u8,
                        difference_of_pic_nums: 0,
                        long_term_pic_num: 0,
                        long_term_frame_idx: 0,
                        max_long_term_frame_idx: 0,
                    };
                    match op {
                        1 | 3 => {
                            m.difference_of_pic_nums = br.ue()? + 1;
                        }
                        _ => {}
                    }
                    if op == 2 {
                        m.long_term_pic_num = br.ue()?;
                    }
                    if op == 3 || op == 6 {
                        m.long_term_frame_idx = br.ue()?;
                    }
                    if op == 4 {
                        m.max_long_term_frame_idx = br.ue()?;
                    }
                    if op == 6 {
                        m.long_term_pic_num = br.ue()?;
                    }
                    if mmco.len() >= 64 {
                        return Err(Error::BadValue("mmco list too long"));
                    }
                    mmco.push(m);
                }
            }
        }

    }
    // cabac_init_idc sits between the marking block and slice_qp_delta
    // on CABAC P/B slices (spec 7.3.3).
    let mut cabac_init_idc = 0;
    if pps.cabac && slice_type != SliceType::I {
        cabac_init_idc = br.ue()?;
        if cabac_init_idc > 2 {
            return Err(Error::BadValue("cabac_init_idc over 2"));
        }
    }

    let slice_qp_delta = br.se()?;
    // disable_deblocking_filter_idc is present only when
    // deblocking_filter_control_present_flag.
    let mut disable_deblock_idc = 0u8;
    let mut offset_a = 0i8;
    let mut offset_b = 0i8;
    if pps.deblocking_control {
        disable_deblock_idc = br.ue()? as u8;
        if disable_deblock_idc > 2 {
            return Err(Error::BadValue("disable_deblocking_filter_idc over 2"));
        }
        if disable_deblock_idc != 1 {
            offset_a = (br.se()? * 2) as i8;
            offset_b = (br.se()? * 2) as i8;
        }
    }

    Ok(SliceHeader {
        first_mb,
        slice_type,
        pps_id,
        frame_num,
        idr_pic_id,
        pic_order_cnt_lsb,
        delta_poc_bottom,
        delta_poc0,
        delta_poc1,
        num_ref_override,
        num_ref_idx_l0_active,
        num_ref_idx_l1_active,
        reorder_l0,
        reorder_l1,
        wp_l0,
        wp_l1,
        wp_denom,
        direct_spatial,
        cabac_init_idc,
        idr_marking,
        adaptive_marking,
        mmco,
        slice_qp_delta,
        disable_deblock_idc,
        offset_a,
        offset_b,
    })
}
