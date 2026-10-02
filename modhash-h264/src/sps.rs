//! Sequence parameter set parsing (spec 7.3.2.1).

use crate::golomb::Br;
use modhash_primitives::{Error, Result};

/// The `profile_idc` values this crate understands, named for error
/// reporting. Every profile outside [`Profile::Baseline`],
/// [`Profile::Main`] and [`Profile::Extended`] is refused with the
/// profile named — P12 decodes the baseline CAVLC feature set only.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Profile {
    /// `profile_idc` 66 (Baseline and Constrained Baseline — the
    /// constraint flags are checked separately).
    Baseline,
    /// `profile_idc` 77 (Main). Decoded so long as the stream only
    /// exercises the CAVLC + I/P-slice subset; CABAC and B-slices
    /// still error when encountered.
    Main,
    /// `profile_idc` 88 (Extended). Same subset as Main for decoding.
    Extended,
}

/// Human-readable name of a `profile_idc`, including profiles this
/// crate refuses, for [`Error::Unsupported`] messages.
pub(crate) fn profile_name(idc: u32) -> &'static str {
    match idc {
        66 => "Baseline",
        77 => "Main",
        88 => "Extended",
        100 => "High",
        110 => "High 10",
        122 => "High 4:2:2",
        244 => "High 4:4:4",
        44 => "CAVLC 4:4:4 Intra",
        83 => "Scalable Baseline",
        86 => "Scalable High",
        118 => "Multiview High",
        128 => "Stereo High",
        _ => "Unknown",
    }
}

/// A parsed sequence parameter set.
#[derive(Clone, Debug)]
pub struct Sps {
    /// `seq_parameter_set_id` (0..=31).
    pub id: u32,
    /// Named profile of `profile_idc`.
    pub profile: Profile,
    /// Raw `profile_idc`.
    pub profile_idc: u8,
    /// Raw `level_idc`.
    pub level_idc: u8,
    /// `log2_max_frame_num_minus4` + 4.
    pub log2_max_frame_num: u32,
    /// `pic_order_cnt_type` (0, 1 or 2).
    pub pic_order_cnt_type: u32,
    /// `log2_max_pic_order_cnt_lsb_minus4` + 4 (type 0).
    pub log2_max_poc_lsb: u32,
    /// `delta_pic_order_always_zero_flag` (type 1).
    pub delta_pic_order_always_zero: bool,
    /// `offset_for_non_ref_pic` (type 1).
    pub offset_for_non_ref_pic: i32,
    /// `offset_for_top_to_bottom_field` (type 1).
    pub offset_for_top_to_bottom: i32,
    /// `num_ref_frames_in_pic_order_cnt_cycle` entries (type 1).
    pub offset_for_ref_frame: alloc::vec::Vec<i32>,
    /// `max_num_ref_frames`.
    pub max_num_ref_frames: u32,
    /// `gaps_in_frame_num_value_allowed_flag`.
    pub gaps_in_frame_num_allowed: bool,
    /// Coded width in macroblocks (`pic_width_in_mbs_minus1` + 1).
    pub width_mbs: u32,
    /// Coded frame height in macroblocks (`pic_height_in_map_units_minus1` + 1).
    pub height_mbs: u32,
    /// `frame_mbs_only_flag` — required by this crate (no interlace).
    pub frame_mbs_only: bool,
    /// `direct_8x8_inference_flag`.
    pub direct_8x8_inference: bool,
    /// `frame_cropping_flag`.
    pub frame_cropping: bool,
    /// Crop offsets `[left, right, top, bottom]` in frame units
    /// (`frame_cropping_*_offset`), already in the 4:2:0 frame scale
    /// (left/right in 2-luma-sample units ×2, i.e. raw syntax values).
    pub crop: [u32; 4],
    /// `vui_parameters_present_flag`; contents are not interpreted.
    pub vui_present: bool,
}

/// Parses an SPS RBSP (payload bytes after the NAL header and after
/// emulation-prevention removal).
///
/// Refuses non-frame content (`frame_mbs_only_flag` == 0), a chroma
/// format other than 4:2:0, and any `profile_idc` whose feature set the
/// decoder cannot cover — naming the profile in the error.
pub(crate) fn parse(payload: &[u8]) -> Result<Sps> {
    let mut b = Br::new(payload);
    let profile_idc = b.bits(8)? as u8;
    let constraint_flags = b.bits(8)?;
    let level_idc = b.bits(8)? as u8;
    let sps_id = b.ue()?;
    if sps_id > 31 {
        return Err(Error::BadValue("seq_parameter_set_id over 31"));
    }

    let profile = match profile_idc {
        66 => Profile::Baseline,
        77 => Profile::Main,
        88 => Profile::Extended,
        other => {
            let name = profile_name(u32::from(other));
            // Leak-free: the error carries a &'static str, so map the
            // profile name back through the table's static strings.
            return Err(Error::Unsupported(match name {
                "High" => "h264 profile: High",
                "High 10" => "h264 profile: High 10",
                "High 4:2:2" => "h264 profile: High 4:2:2",
                "High 4:4:4" => "h264 profile: High 4:4:4",
                "CAVLC 4:4:4 Intra" => "h264 profile: CAVLC 4:4:4 Intra",
                "Scalable Baseline" => "h264 profile: Scalable Baseline",
                "Scalable High" => "h264 profile: Scalable High",
                "Multiview High" => "h264 profile: Multiview High",
                "Stereo High" => "h264 profile: Stereo High",
                _ => "h264 profile: unrecognised",
            }));
        }
    };
    // Constrained baseline is baseline syntax: constraint_set1_flag set
    // simply guarantees that. No action needed beyond the flag parse.

    // High-family profiles carry extra SPS fields; they are refused by
    // the match above, so chroma_format_idc stays implicitly 1 (4:2:0).
    let log2_max_frame_num = b.ue()? + 4;
    if !(4..=16).contains(&log2_max_frame_num) {
        return Err(Error::BadValue("log2_max_frame_num out of range"));
    }
    let pic_order_cnt_type = b.ue()?;
    let mut log2_max_poc_lsb = 0;
    let mut delta_pic_order_always_zero = false;
    let mut offset_for_non_ref_pic = 0;
    let mut offset_for_top_to_bottom = 0;
    let mut offset_for_ref_frame = alloc::vec::Vec::new();
    match pic_order_cnt_type {
        0 => {
            log2_max_poc_lsb = b.ue()? + 4;
            if !(4..=16).contains(&log2_max_poc_lsb) {
                return Err(Error::BadValue("log2_max_pic_order_cnt_lsb out of range"));
            }
        }
        1 => {
            delta_pic_order_always_zero = b.bit()?;
            offset_for_non_ref_pic = b.se()?;
            offset_for_top_to_bottom = b.se()?;
            let cycles = b.ue()?;
            if cycles > 255 {
                return Err(Error::BadValue(
                    "num_ref_frames_in_pic_order_cnt_cycle over 255",
                ));
            }
            for _ in 0..cycles {
                offset_for_ref_frame.push(b.se()?);
            }
        }
        2 => {}
        _ => return Err(Error::BadValue("pic_order_cnt_type over 2")),
    }

    let max_num_ref_frames = b.ue()?;
    let gaps_in_frame_num_allowed = b.bit()?;
    let width_mbs = b.ue()? + 1;
    let height_mbs = b.ue()? + 1;
    if width_mbs > 2048 || height_mbs > 2048 {
        return Err(Error::BadValue("coded picture size absurd"));
    }
    let frame_mbs_only = b.bit()?;
    if !frame_mbs_only {
        return Err(Error::Unsupported(
            "h264 interlaced (frame_mbs_only_flag = 0)",
        ));
    }
    let direct_8x8_inference = b.bit()?;
    let frame_cropping = b.bit()?;
    let mut crop = [0u32; 4];
    if frame_cropping {
        for c in &mut crop {
            *c = b.ue()?;
        }
        // 4:2:0 frame crops in units of 2 luma samples horizontally and
        // vertically; offsets are in those units, so width loss is
        // (left+right)*2. Reject crops that erase the picture.
        let w = width_mbs * 16;
        let h = height_mbs * 16;
        if (crop[0] + crop[1]) * 2 >= w || (crop[2] + crop[3]) * 2 >= h {
            return Err(Error::BadValue("frame crop exceeds coded size"));
        }
    }
    let vui_present = b.bit()?;
    // vui_parameters() is parsed only far enough to not misread the
    // tail: it is always the last field before rbsp_trailing_bits, so
    // skipping it entirely is correct.
    let _ = vui_present;
    let _ = constraint_flags;
    let _ = level_idc;

    Ok(Sps {
        id: sps_id,
        profile,
        profile_idc,
        level_idc,
        log2_max_frame_num,
        pic_order_cnt_type,
        log2_max_poc_lsb,
        delta_pic_order_always_zero,
        offset_for_non_ref_pic,
        offset_for_top_to_bottom,
        offset_for_ref_frame,
        max_num_ref_frames,
        gaps_in_frame_num_allowed,
        width_mbs,
        height_mbs,
        frame_mbs_only,
        direct_8x8_inference,
        frame_cropping,
        crop,
        vui_present,
    })
}
