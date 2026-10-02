//! Picture parameter set parsing (spec 7.3.2.2).

use crate::golomb::Br;
use modhash_primitives::{Error, Result};

/// A parsed picture parameter set.
#[derive(Clone, Debug)]
pub(crate) struct Pps {
    /// `pic_parameter_set_id` (0..=255).
    pub id: u32,
    /// Referenced `seq_parameter_set_id`.
    pub sps_id: u32,
    /// `entropy_coding_mode_flag` — `false` (CAVLC) required.
    pub cabac: bool,
    /// `pic_order_present_flag` (bottom-field POC delta in slice headers).
    pub bottom_field_pic_order: bool,
    /// `num_ref_idx_l0_active_minus1` + 1.
    pub num_ref_idx_l0_active: u32,
    /// `num_ref_idx_l1_active_minus1` + 1 (B-slices only).
    pub num_ref_idx_l1_active: u32,
    /// `weighted_pred_flag` (P-slice weighted prediction).
    pub weighted_pred: bool,
    /// `weighted_bipred_idc` (B-slices only).
    pub weighted_bipred_idc: u32,
    /// `pic_init_qp_minus26` + 26.
    pub pic_init_qp: i32,
    /// `pic_init_qs_minus26` + 26.
    pub pic_init_qs: i32,
    /// `chroma_qp_index_offset`.
    pub chroma_qp_index_offset: i32,
    /// `deblocking_filter_control_present_flag`.
    pub deblocking_control: bool,
    /// `constrained_intra_pred_flag`.
    pub constrained_intra_pred: bool,
    /// `redundant_pic_cnt_present_flag` — required `false`.
    pub redundant_pic_cnt: bool,
}

/// Parses a PPS RBSP. `have_more_rbsp` — this crate parses the
/// transform_8x8 tail unconditionally per spec (it is present when
/// `more_rbsp_data` is true).
pub(crate) fn parse(payload: &[u8]) -> Result<Pps> {
    let mut b = Br::new(payload);
    let id = b.ue()?;
    if id > 255 {
        return Err(Error::BadValue("pic_parameter_set_id over 255"));
    }
    let sps_id = b.ue()?;
    if sps_id > 31 {
        return Err(Error::BadValue("pps references sps id over 31"));
    }
    let cabac = b.bit()?;
    if cabac {
        return Err(Error::Unsupported("h264 CABAC (entropy_coding_mode_flag = 1)"));
    }
    let bottom_field_pic_order = b.bit()?;
    let num_slice_groups = b.ue()? + 1;
    if num_slice_groups > 1 {
        return Err(Error::Unsupported("h264 FMO (num_slice_groups_minus1 > 0)"));
    }
    let num_ref_idx_l0_active = b.ue()? + 1;
    let num_ref_idx_l1_active = b.ue()? + 1;
    if num_ref_idx_l0_active > 32 || num_ref_idx_l1_active > 32 {
        return Err(Error::BadValue("num_ref_idx_active over 32"));
    }
    let weighted_pred = b.bit()?;
    let weighted_bipred_idc = b.bits(2)?;
    let pic_init_qp = b.se()? + 26;
    let pic_init_qs = b.se()? + 26;
    if !(0..=51).contains(&pic_init_qp) || !(0..=51).contains(&pic_init_qs) {
        return Err(Error::BadValue("pic_init_qp/qs out of range"));
    }
    let chroma_qp_index_offset = b.se()?;
    if !(-12..=12).contains(&chroma_qp_index_offset) {
        return Err(Error::BadValue("chroma_qp_index_offset out of range"));
    }
    let deblocking_control = b.bit()?;
    let constrained_intra_pred = b.bit()?;
    let redundant_pic_cnt = b.bit()?;
    if redundant_pic_cnt {
        return Err(Error::Unsupported(
            "h264 redundant pictures (redundant_pic_cnt_present_flag)",
        ));
    }

    // Extension tail (more_rbsp_data): transform_8x8_mode_flag,
    // pic_scaling_matrix, second chroma offset. Baseline streams do not
    // send it; when present and it asks for 8x8 transforms that is a
    // refusal, not a silent skip.
    if !b.no_more_rbsp_data() {
        let transform_8x8 = b.bit()?;
        if transform_8x8 {
            return Err(Error::Unsupported("h264 transform_8x8_mode (High profile)"));
        }
        let scaling_matrix = b.bit()?;
        if scaling_matrix {
            // Custom scaling lists are a High-profile feature; even
            // the flat-defaults form changes quantisation, so this is
            // always a refusal.
            return Err(Error::Unsupported("h264 pic_scaling_matrix (High profile)"));
        }
        let _second_chroma_offset = b.se()?;
    }

    Ok(Pps {
        id,
        sps_id,
        cabac,
        bottom_field_pic_order,
        num_ref_idx_l0_active,
        num_ref_idx_l1_active,
        weighted_pred,
        weighted_bipred_idc,
        pic_init_qp,
        pic_init_qs,
        chroma_qp_index_offset,
        deblocking_control,
        constrained_intra_pred,
        redundant_pic_cnt,
    })
}
