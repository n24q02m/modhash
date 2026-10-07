//! The decode driver: Annex-B NAL loop → SPS/PPS state → slice-data
//! macroblock layer → intra/inter reconstruction → deblocking → DPB
//! insertion → [`Frame`] output.
//!
//! Scope is the baseline subset the rest of the crate implements:
//! I/P slices, CAVLC, frame pictures (`frame_mbs_only` enforced in the
//! SPS parse), 4:2:0 chroma. Field pictures, MBAFF, B/SP/SI slices,
//! CABAC and data partitioning surface as [`Error::Unsupported`];
//! malformed streams surface as [`Error`] instead of panicking.

/// Debug trace macro (test builds only; compiles away otherwise).
use crate::cabac::{self, ResCat};
use crate::cavlc::{self, BlockKind};
use crate::deblock;
use crate::dpb::{ColocMb, Dpb};
use crate::golomb::Br;
use crate::inter;
use crate::intra::{self, NbSamples};
use crate::mb::{MbMap, MbState, MbType, Nb, SubMbType, intra4x4_neighbours};
use crate::nal::{self, Nal};
use crate::pps::{self, Pps};
use crate::slice::{self, Mmco, SliceHeader, SliceType};
use crate::sps::{self, Sps};
use crate::tables::{
    CAVLC_SCAN_8X8, CBP_INTER, CBP_INTRA, QPC_TABLE, SCAN_2X2, ZIGZAG_4X4, ZIGZAG_8X8, block_index,
    block_xy,
};
use crate::transform;
use crate::{Frame, Limits};
use alloc::vec::Vec;
use modhash_primitives::{Error, Result};

/// Coded planes of one in-progress picture. Luma stride `w`, chroma
/// stride `w/2`; both full-macroblock aligned.
#[derive(Clone)]
struct FrameBuf {
    w: usize,
    h: usize,
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
}

impl FrameBuf {
    fn new(w: usize, h: usize) -> FrameBuf {
        FrameBuf {
            w,
            h,
            y: alloc::vec![0u8; w * h],
            cb: alloc::vec![0u8; w * h / 4],
            cr: alloc::vec![0u8; w * h / 4],
        }
    }
}

/// Per-picture decode state.
struct Pic {
    buf: FrameBuf,
    /// Per-MB state (picture order).
    mbs: Vec<MbState>,
    /// Which SPS/PPS this picture is coded under.
    sps_id: u32,
    pps_id: u32,
    /// PPS chroma QP offsets at picture creation (deblocking input;
    /// kept on the picture so a mid-stream PPS switch cannot rewrite
    /// an in-flight picture's filter state).
    chroma_off_cb: i32,
    chroma_off_cr: i32,
    /// `frame_num` of the picture (slice-boundary detection).
    frame_num: u32,
    /// Picture order count of this picture (spec 8.2.1), needed for
    /// RefPicList construction and temporal-direct scaling.
    poc: i64,
    /// `nal_ref_idc` of its slices.
    nal_ref_idc: u8,
    /// Whether the picture is IDR, and its `idr_pic_id` (multi-slice
    /// IDR pictures must repeat the same id).
    idr: bool,
    idr_pic_id: u32,
    /// Deferred marking: `(adaptive, idr_marking, mmco)` of the
    /// picture's last slice.
    marking: (bool, (bool, bool), Vec<Mmco>),
}

/// Decoder state carried across NALs of one `decode` call.
///
/// Exposed so [`crate::decode`] can be a thin wrapper; a caller wanting
/// push-based decoding can drive [`Decoder::push_stream`] itself.
pub struct Decoder {
    limits: Limits,
    sps: Option<Sps>,
    pps: Option<Pps>,
    dpb: Dpb,
    pic: Option<Pic>,
    frames: Vec<Frame>,
    /// Display-order sort keys parallel to `frames`: `(epoch, poc)`
    /// where `epoch` bumps on every IDR so per-GOP POC restarts sort
    /// inside their own GOP (B-slice streams decode out of order).
    frame_pocs: Vec<(i64, i64)>,
    /// Current POC epoch — incremented by every decoded IDR picture.
    poc_epoch: i64,
    /// Monotonic slice counter feeding `MbState::slice_id`.
    slice_seq: u32,
    /// POC bookkeeping for `pic_order_cnt_type` 0: MSB/LSB of the
    /// previous reference picture (spec 8.2.1.1).
    prev_poc_msb: i64,
    prev_poc_lsb: i64,
    /// `frame_num` of the previous reference picture (POC types 1/2).
    prev_frame_num: u32,
    /// `frameNumOffset` accumulator of POC type 1.
    prev_frame_num_offset: i64,
    /// `frame_num`s of the last decoded picture's RefPicList0/L1 —
    /// stored on the reference at `finish_picture` so temporal direct
    /// can map colocated ref indices back (spec 8.4.1.2).
    last_lists: (Vec<u32>, Vec<u32>),
}

/// Per-slice state shared by every macroblock call.
struct SliceCx<'a> {
    sps: &'a Sps,
    pps: &'a Pps,
    h: &'a SliceHeader,
    /// Resolved reference list 0: indices into `dpb.refs` in list order.
    ref_order: &'a [usize],
    /// Resolved reference list 1 (B slices only).
    ref_order1: &'a [usize],
    /// `QPy` — updated by `mb_qp_delta`; skip MBs keep it untouched
    /// (spec 7.4.5: `QPy` carries through `mb_skip_run`).
    qp_prev: i32,
    /// Previous MB's `mb_qp_delta` — the CABAC `mb_qp_delta` context
    /// splits on it being zero (spec 9.3.2.7); skipped/direct MBs reset it.
    qp_delta_prev: i32,
    /// Slice id for `MbState::slice_id`.
    slice_id: u32,
    /// Implicit weighted-prediction `w0` table indexed
    /// `[ref0][ref1]` (built only under `weighted_bipred_idc == 2`;
    /// entries 32 when unused). Luma and chroma share it.
    iw: &'a [i32],
    /// Width of `iw` (= number of L1 entries).
    iw_w: usize,
    /// POC of the picture being decoded (implicit-weight and
    /// temporal-direct scaling).
    cur_poc: i64,
    /// `frame_num`s of `ref_order` slots — the temporal-direct
    /// colocation map needs list->frame identity.
    ref_l0_fns: &'a [u32],
}

impl SliceCx<'_> {
    /// Implicit `w0` weight for the (ref0, ref1) bi-prediction pair.
    fn iw0(&self, ref0: usize, ref1: usize) -> i32 {
        self.iw.get(ref0 * self.iw_w + ref1).copied().unwrap_or(32)
    }
}

impl Decoder {
    /// Creates a decoder honouring `limits`.
    pub fn new(limits: Limits) -> Decoder {
        Decoder {
            limits,
            sps: None,
            pps: None,
            dpb: Dpb::default(),
            pic: None,
            frames: Vec::new(),
            frame_pocs: Vec::new(),
            poc_epoch: 0,
            slice_seq: 0,
            prev_poc_msb: 0,
            prev_poc_lsb: 0,
            prev_frame_num: 0,
            prev_frame_num_offset: 0,
            last_lists: (Vec::new(), Vec::new()),
        }
    }

    /// Top-level POC derivation (spec 8.2.1): wraps the type-specific
    /// computation and the prev-picture state updates the spec makes
    /// conditional on `nal_ref_idc != 0`. Called once per new picture,
    /// using the *first slice's* header values.
    fn poc_of(&mut self, h: &SliceHeader, sps: &Sps, nal_ref_idc: u8, idr: bool) -> i64 {
        let poc = compute_poc(
            h,
            sps,
            nal_ref_idc,
            idr,
            self.prev_poc_msb,
            self.prev_poc_lsb,
            self.prev_frame_num,
            self.prev_frame_num_offset,
        );
        if nal_ref_idc != 0 {
            self.prev_frame_num = h.frame_num;
            self.prev_frame_num_offset = poc.1;
            self.prev_poc_msb = poc.2;
            self.prev_poc_lsb = poc.3;
        }
        poc.0
    }

    /// Feeds a whole Annex-B stream; returns decoded frames in order.
    pub fn push_stream(&mut self, stream: &[u8]) -> Result<Vec<Frame>> {
        let nals = nal::split_annex_b(stream)?;
        if nals.is_empty() {
            return Err(Error::BadValue("h264 stream with no NAL units"));
        }
        for n in &nals {
            self.nal(n)?;
        }
        self.end_of_stream()
    }

    fn nal(&mut self, n: &Nal<'_>) -> Result<()> {
        match n.unit_type {
            nal::NAL_SPS => {
                let rbsp = nal::rbsp(n.payload)?;
                let sps = sps::parse(&rbsp)?;
                self.dpb.reset(
                    1u32 << sps.log2_max_frame_num,
                    sps.max_num_ref_frames,
                    self.limits.max_refs,
                );
                let coded = (sps.width_mbs * 16) as u64 * (sps.height_mbs * 16) as u64;
                if coded > self.limits.max_luma_samples as u64 {
                    return Err(Error::too_large(
                        "h264 picture",
                        self.limits.max_luma_samples,
                    ));
                }
                self.sps = Some(sps);
                self.pps = None;
            }
            nal::NAL_PPS => {
                if self.sps.is_none() {
                    return Err(Error::BadValue("PPS before any SPS"));
                }
                let rbsp = nal::rbsp(n.payload)?;
                let p = pps::parse(&rbsp)?;
                let sps = self.sps.as_ref().unwrap();
                if p.sps_id != sps.id {
                    return Err(Error::BadValue("PPS references unknown SPS"));
                }
                self.pps = Some(p);
            }
            nal::NAL_SLICE | nal::NAL_IDR => self.slice_nal(n, n.unit_type == nal::NAL_IDR)?,
            nal::NAL_DPA | nal::NAL_DPB | nal::NAL_DPC => {
                return Err(Error::Unsupported("h264 slice data partitioning"));
            }
            // Non-VCL NALs this crate does not decode — skipped.
            nal::NAL_SEI
            | nal::NAL_AUD
            | nal::NAL_EOSEQ
            | nal::NAL_EOSTREAM
            | nal::NAL_FILLER
            | nal::NAL_SPS_EXT => {}
            // Auxiliary/extension/scalable slice types are real
            // picture data we refuse, not padding.
            nal::NAL_CODED_SLICE_AUX | 14 | 15 | 20 | 21 => {
                return Err(Error::Unsupported("h264 auxiliary/extension slice"));
            }
            _ => {}
        }
        Ok(())
    }

    /// One VCL NAL: header parse, picture-boundary detection, MB loop.
    fn slice_nal(&mut self, n: &Nal<'_>, idr: bool) -> Result<()> {
        let sps = self
            .sps
            .as_ref()
            .ok_or(Error::BadValue("slice before SPS"))?
            .clone();
        let pps = self
            .pps
            .as_ref()
            .ok_or(Error::BadValue("slice before PPS"))?
            .clone();
        let rbsp = nal::rbsp(n.payload)?;
        let mut br = Br::new(&rbsp);
        let h = slice::parse_header(&mut br, idr, n.ref_idc, &sps, &pps)?;
        self.slice_seq = self.slice_seq.wrapping_add(1);
        let slice_id = self.slice_seq;

        let new_pic = match &self.pic {
            Some(p) => {
                p.sps_id != sps.id
                    || p.pps_id != h.pps_id
                    || p.idr != idr
                    || p.frame_num != h.frame_num
                    || (h.first_mb == 0 && p.mbs[0].slice_id != u32::MAX)
                    || (idr && p.idr && p.idr_pic_id != h.idr_pic_id)
            }
            None => true,
        };
        let cur_poc = if new_pic {
            Some(self.poc_of(&h, &sps, n.ref_idc, idr))
        } else {
            None
        };
        if new_pic {
            self.finish_picture()?;
            if idr {
                // IDR flushes the DPB unless the header asks for
                // long-term keeping (spec 8.2.5.1).
                if !h.idr_marking.0 {
                    self.dpb.flush();
                }
            }
            let w = (sps.width_mbs * 16) as usize;
            let hh = (sps.height_mbs * 16) as usize;
            let poc = cur_poc.unwrap_or(0);
            self.pic = Some(Pic {
                buf: FrameBuf::new(w, hh),
                mbs: alloc::vec![MbState::new(); (sps.width_mbs * sps.height_mbs) as usize],
                sps_id: sps.id,
                pps_id: h.pps_id,
                chroma_off_cb: pps.chroma_qp_index_offset,
                chroma_off_cr: pps.chroma_qp_index_offset_cr,
                frame_num: h.frame_num,
                poc,
                nal_ref_idc: n.ref_idc,
                idr,
                idr_pic_id: h.idr_pic_id,
                marking: (false, h.idr_marking, Vec::new()),
            });
        }
        if let Some(p) = self.pic.as_mut() {
            p.nal_ref_idc = n.ref_idc;
            if h.adaptive_marking || !h.mmco.is_empty() {
                p.marking = (h.adaptive_marking, h.idr_marking, h.mmco.clone());
            }
        }
        let is_b = h.slice_type == SliceType::B;
        let ref_order: Vec<usize> = if h.slice_type != SliceType::I {
            let order = self
                .dpb
                .ref_list0(&h, h.frame_num, pic_poc(&self.pic), is_b)?;
            if order.is_empty() {
                return Err(Error::BadValue("P/B slice with empty L0 list"));
            }
            order
        } else {
            Vec::new()
        };
        let ref_order1: Vec<usize> = if is_b {
            let order = self
                .dpb
                .ref_list1(&h, h.frame_num, pic_poc(&self.pic), &ref_order)?;
            if order.is_empty() {
                return Err(Error::BadValue("B slice with empty L1 list"));
            }
            order
        } else {
            Vec::new()
        };
        // Record this picture's reference lists (frame_nums) for the
        // colocation mapping — only ref slices produce lists.
        if h.slice_type != SliceType::I {
            let map = |order: &[usize]| -> Result<alloc::vec::Vec<u32>> {
                order
                    .iter()
                    .map(|&i| {
                        self.dpb
                            .refs
                            .get(i)
                            .map(|r| r.frame_num)
                            .ok_or(Error::BadValue("ref list index out of range"))
                    })
                    .collect()
            };
            self.last_lists = (map(&ref_order)?, map(&ref_order1)?);
        }
        // Implicit weighted-prediction table (weighted_bipred_idc==2):
        // w0 for every (L0, L1) ref pair. Chroma shares the luma
        // denominators (log2=5).
        let iw: Vec<i32> = if is_b && pps.weighted_bipred_idc == 2 {
            let cur = pic_poc(&self.pic);
            let mut v = alloc::vec![32i32; ref_order.len() * ref_order1.len()];
            for (i0, &r0) in ref_order.iter().enumerate() {
                for (i1, &r1) in ref_order1.iter().enumerate() {
                    v[i0 * ref_order1.len() + i1] =
                        implicit_w0(cur, self.dpb.refs[r0].poc, self.dpb.refs[r1].poc);
                }
            }
            v
        } else {
            Vec::new()
        };
        let l0_fns: Vec<u32> = ref_order
            .iter()
            .map(|&i| self.dpb.refs[i].frame_num)
            .collect();

        let mut cx = SliceCx {
            sps: &sps,
            pps: &pps,
            h: &h,
            ref_order: &ref_order,
            ref_order1: &ref_order1,
            qp_prev: pps.pic_init_qp + h.slice_qp_delta,
            qp_delta_prev: 0,
            slice_id,
            iw: &iw,
            iw_w: ref_order1.len().max(1),
            cur_poc: pic_poc(&self.pic),
            ref_l0_fns: &l0_fns,
        };
        let pic = self.pic.as_mut().unwrap();
        slice_data(pic, &mut br, &mut cx, &self.dpb, &rbsp)
    }

    /// Closes the current picture: deblock, DPB insert + marking, crop,
    /// output.
    fn finish_picture(&mut self) -> Result<()> {
        let Some(pic) = self.pic.take() else {
            return Ok(());
        };
        let sps = self
            .sps
            .clone()
            .ok_or(Error::BadValue("picture without SPS"))?;
        let wm = sps.width_mbs as usize;
        let hm = sps.height_mbs as usize;
        let mut buf = pic.buf;

        deblock::filter_frame(
            &mut buf.y,
            &mut buf.cb,
            &mut buf.cr,
            buf.w,
            &pic.mbs,
            wm,
            hm,
            &QPC_TABLE,
            pic.chroma_off_cb,
            pic.chroma_off_cr,
        );

        if pic.nal_ref_idc != 0 {
            // Colocation data for future B-slice temporal direct
            // prediction: the picture's own L0/L1 lists (as frame_nums)
            // plus per-MB mv/ref snapshots.
            let coloc: Vec<ColocMb> = pic
                .mbs
                .iter()
                .map(|m| ColocMb {
                    mv_l0: m.mv,
                    mv_l1: m.mv_l1,
                    ref_l0: [
                        m.ref_idx[block_index(0, 0)],
                        m.ref_idx[block_index(2, 0)],
                        m.ref_idx[block_index(0, 2)],
                        m.ref_idx[block_index(2, 2)],
                    ],
                    ref_l1: [
                        m.ref_idx_l1[block_index(0, 0)],
                        m.ref_idx_l1[block_index(2, 0)],
                        m.ref_idx_l1[block_index(0, 2)],
                        m.ref_idx_l1[block_index(2, 2)],
                    ],
                    intra: m.mb_type.is_intra(),
                })
                .collect();
            // The lists this picture used (only the most recent
            // non-I slices matter — tracked via `last_lists`).
            let (rl0, rl1) = self.last_lists.clone();
            self.dpb.push(
                pic.frame_num,
                pic.poc,
                buf.y.clone(),
                buf.cb.clone(),
                buf.cr.clone(),
                coloc,
                rl0,
                rl1,
            );
            // Synthesised header carrying the picture's deferred
            // marking — `apply_marking` reads only these fields.
            let stub = SliceHeader {
                first_mb: 0,
                slice_type: SliceType::I,
                pps_id: pic.pps_id,
                frame_num: pic.frame_num,
                idr_pic_id: 0,
                pic_order_cnt_lsb: 0,
                delta_poc_bottom: 0,
                delta_poc0: 0,
                delta_poc1: 0,
                num_ref_override: false,
                num_ref_idx_l0_active: 1,
                num_ref_idx_l1_active: 1,
                reorder_l0: Vec::new(),
                reorder_l1: Vec::new(),
                wp_l0: None,
                wp_l1: None,
                wp_denom: (0, 0),
                direct_spatial: false,
                cabac_init_idc: 0,
                idr_marking: pic.marking.1,
                adaptive_marking: pic.marking.0,
                mmco: pic.marking.2.clone(),
                slice_qp_delta: 0,
                disable_deblock_idc: 0,
                offset_a: 0,
                offset_b: 0,
            };
            self.dpb
                .apply_marking(&stub, pic.frame_num, pic.nal_ref_idc)?;
            // MMCO op 5 (spec 8.2.5.4.6): re-base POC — the just-
            // finished picture becomes the wrap reference.
            if pic.marking.0 && pic.marking.2.iter().any(|m| m.op == 5) {
                self.prev_frame_num = 0;
                self.prev_frame_num_offset = 0;
                self.prev_poc_msb = 0;
                self.prev_poc_lsb = i64::from(stub.pic_order_cnt_lsb);
            }
        }
        if self.frames.len() >= self.limits.max_frames {
            return Err(Error::too_large("h264 frames", self.limits.max_frames));
        }
        if pic.idr {
            self.poc_epoch += 1;
        }
        self.frame_pocs.push((self.poc_epoch, pic.poc));
        self.frames.push(crop_frame(&sps, &buf)?);
        Ok(())
    }

    /// End-of-stream: flushes the in-progress picture (if any) and
    /// returns every decoded frame in presentation order.
    /// debug
    fn end_of_stream(&mut self) -> Result<Vec<Frame>> {
        self.finish_picture()?;
        // Presentation order = ascending POC. `sort_by_key` is stable
        // so same-POC pairs (shouldn't occur for frame streams) keep
        // decode order.
        let mut order: Vec<usize> = (0..self.frames.len()).collect();
        order.sort_by_key(|&i| self.frame_pocs[i]);
        let mut out = Vec::with_capacity(self.frames.len());
        for &i in order.iter() {
            let f = &self.frames[i];
            out.push(f.clone());
        }
        Ok(out)
    }
}

/// Picture-order-count derivation (spec 8.2.1), ported from the
/// reference decoder's `ff_h264_init_poc`. Returns
/// `(poc, frame_num_offset_used, poc_msb_used, poc_lsb_used)` — the
/// "used" values are what the spec stores back for non-reference
/// pictures (IDR resets them to the current values, MMCO-5 to the
/// picture's own).
#[allow(clippy::too_many_arguments)]
fn compute_poc(
    h: &SliceHeader,
    sps: &Sps,
    nal_ref_idc: u8,
    idr: bool,
    prev_msb: i64,
    prev_lsb: i64,
    prev_fn: u32,
    prev_fno: i64,
) -> (i64, i64, i64, i64) {
    let max_frame_num = 1i64 << sps.log2_max_frame_num;
    let mut frame_num_offset = prev_fno;
    if i64::from(h.frame_num) < i64::from(prev_fn) {
        frame_num_offset += max_frame_num;
    }
    if idr {
        // IDR resets the wrap reference to this picture's own lsb.
        let lsb = i64::from(h.pic_order_cnt_lsb);
        let top = lsb + i64::from(h.delta_poc_bottom);
        return (top, 0, 0, lsb);
    }
    match sps.pic_order_cnt_type {
        0 => {
            let max_lsb = 1i64 << sps.log2_max_poc_lsb;
            let lsb = i64::from(h.pic_order_cnt_lsb);
            // prev_lsb == -1 marks "no previous reference yet" (fresh
            // decoder or just after IDR): adopt the current lsb.
            let plsb = if prev_lsb < 0 { lsb } else { prev_lsb };
            let msb = if lsb < plsb && plsb - lsb >= max_lsb / 2 {
                prev_msb + max_lsb
            } else if lsb > plsb && plsb - lsb < -max_lsb / 2 {
                prev_msb - max_lsb
            } else {
                prev_msb
            };
            let top = msb + lsb;
            let bottom = top + i64::from(h.delta_poc_bottom);
            (top.min(bottom), frame_num_offset, msb, lsb)
        }
        1 => {
            // Spec 8.2.1.2 as implemented by the reference decoder.
            let mut abs_frame_num = if sps.offset_for_ref_frame.is_empty() {
                0i64
            } else {
                frame_num_offset + i64::from(h.frame_num)
            };
            if nal_ref_idc == 0 && abs_frame_num > 0 {
                abs_frame_num -= 1;
            }
            let cycle_len = sps.offset_for_ref_frame.len() as i64;
            let expected_delta: i64 = sps.offset_for_ref_frame.iter().map(|&v| i64::from(v)).sum();
            let mut expected_poc = 0i64;
            if abs_frame_num > 0 && cycle_len > 0 {
                let poc_cycle_cnt = (abs_frame_num - 1) / cycle_len;
                let frame_num_in_cycle = (abs_frame_num - 1) % cycle_len;
                expected_poc = poc_cycle_cnt * expected_delta;
                for (i, &d) in sps.offset_for_ref_frame.iter().enumerate() {
                    if i as i64 > frame_num_in_cycle {
                        break;
                    }
                    expected_poc += i64::from(d);
                }
            }
            if nal_ref_idc == 0 {
                expected_poc += i64::from(sps.offset_for_non_ref_pic);
            }
            let top = expected_poc + i64::from(h.delta_poc0);
            let bottom = top + i64::from(sps.offset_for_top_to_bottom) + i64::from(h.delta_poc1);
            (top.min(bottom), frame_num_offset, 0, 0)
        }
        _ => {
            // Type 2 (spec 8.2.1.3): poc = 2*(frameNumOffset+frame_num)
            // minus one for non-reference pictures.
            let mut poc = 2 * (frame_num_offset + i64::from(h.frame_num));
            if nal_ref_idc == 0 {
                poc -= 1;
            }
            (poc, frame_num_offset, 0, 0)
        }
    }
}

/// `Pic::poc` of the picture in progress (set at creation).
fn pic_poc(pic: &Option<Pic>) -> i64 {
    pic.as_ref().map(|p| p.poc).unwrap_or(0)
}

/// Crops a coded [`FrameBuf`] to the display rectangle from the SPS and
/// packs it into a [`Frame`]. `Sps::crop` holds the four
/// `frame_cropping_*_offset` values already scaled to frame units;
/// chroma offsets are `offset` samples in each halved plane.
fn crop_frame(sps: &Sps, buf: &FrameBuf) -> Result<Frame> {
    let coded_w = sps.width_mbs * 16;
    let coded_h = sps.height_mbs * 16;
    let (cl, cr_, ct, cb_) = (sps.crop[0], sps.crop[1], sps.crop[2], sps.crop[3]);
    // 4:2:0 frame crop units are 2 luma samples horizontally and
    // vertically (spec Table 7-3 / 7.4.2.1 crop unit scale).
    let w = coded_w
        .checked_sub((cl + cr_) * 2)
        .ok_or(Error::BadValue("crop width"))?;
    let h = coded_h
        .checked_sub((ct + cb_) * 2)
        .ok_or(Error::BadValue("crop height"))?;
    let (w, h) = (w as usize, h as usize);
    let (x0, y0) = (cl as usize * 2, ct as usize * 2);
    if w == 0 || h == 0 || x0 + w > buf.w || y0 + h > buf.h {
        return Err(Error::BadValue("h264 crop window outside frame"));
    }
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let mut out = Frame {
        width: w as u32,
        height: h as u32,
        y: alloc::vec![0u8; w * h],
        cb: alloc::vec![0u8; cw * ch],
        cr: alloc::vec![0u8; cw * ch],
    };
    for r in 0..h {
        out.y[r * w..r * w + w]
            .copy_from_slice(&buf.y[(y0 + r) * buf.w + x0..(y0 + r) * buf.w + x0 + w]);
    }
    let (cx0, cy0) = (x0 / 2, y0 / 2);
    for r in 0..ch {
        out.cb[r * cw..r * cw + cw].copy_from_slice(
            &buf.cb[(cy0 + r) * (buf.w / 2) + cx0..(cy0 + r) * (buf.w / 2) + cx0 + cw],
        );
        out.cr[r * cw..r * cw + cw].copy_from_slice(
            &buf.cr[(cy0 + r) * (buf.w / 2) + cx0..(cy0 + r) * (buf.w / 2) + cx0 + cw],
        );
    }
    Ok(out)
}

/// Same-slice neighbour fetch: `None` on picture edge or a
/// different-slice MB (CABAC ctx counts it as unavailable).
fn nb_at(mbs: &[MbState], i: Option<usize>, sid: u32) -> Option<&MbState> {
    i.and_then(|j| mbs.get(j)).filter(|m| m.slice_id == sid)
}

/// The macroblock-layer loop of `slice_data` (spec 7.3.4).
fn slice_data(
    pic: &mut Pic,
    br: &mut Br<'_>,
    cx: &mut SliceCx<'_>,
    dpb: &Dpb,
    rbsp: &[u8],
) -> Result<()> {
    if cx.pps.cabac {
        // `cabac_alignment_one_bit` pads the header to the next byte
        // boundary before `cabac_slice_data` begins (spec 7.3.4).
        br.byte_align();
        let off = br.position() / 8;
        return slice_data_cabac(pic, cx, dpb, rbsp, off);
    }
    let wm = cx.sps.width_mbs as usize;
    let total = (cx.sps.width_mbs * sps_height(cx)) as usize;
    let mut mb_idx = cx.h.first_mb as usize;
    if mb_idx > total {
        return Err(Error::BadValue("first_mb_in_slice past picture"));
    }
    loop {
        if cx.h.slice_type == SliceType::P {
            let run = br.ue()?;
            for _ in 0..run {
                if mb_idx >= total {
                    return Err(Error::BadValue("mb_skip_run past picture"));
                }
                p_skip(pic, cx, dpb, mb_idx, wm)?;
                mb_idx += 1;
            }
            if mb_idx >= total || br.no_more_rbsp_data() {
                break;
            }
        }
        if cx.h.slice_type == SliceType::B {
            let run = br.ue()?;
            for _ in 0..run {
                if mb_idx >= total {
                    return Err(Error::BadValue("mb_skip_run past picture"));
                }
                b_skip(pic, cx, dpb, mb_idx, wm)?;
                mb_idx += 1;
            }
            if mb_idx >= total || br.no_more_rbsp_data() {
                break;
            }
        }
        mb_decode(pic, br, cx, dpb, mb_idx, wm)?;
        mb_idx += 1;
        if mb_idx >= total || br.no_more_rbsp_data() {
            break;
        }
    }
    Ok(())
}

/// `sps.height_mbs` through the slice context (one-liner to keep the
/// two loops readable).
fn sps_height(cx: &SliceCx<'_>) -> u32 {
    cx.sps.height_mbs
}

/// P_Skip reconstruction (spec 8.4.1 / 7.3.5.2): one 16x16 partition,
/// `ref_idx` 0, MV = the median predictor, no residual.
fn p_skip(pic: &mut Pic, cx: &mut SliceCx<'_>, dpb: &Dpb, idx: usize, wm: usize) -> Result<()> {
    let map = MbMap {
        idx,
        x: idx % wm,
        y: idx / wm,
        width: wm,
        sid: cx.slice_id,
    };
    if cx.ref_order.is_empty() {
        return Err(Error::BadValue("P_Skip with empty ref list"));
    }
    // Spec 8.4.1.1: the skip motion vector is the median predictor,
    // EXCEPT it is forced to (0,0) when the left or top neighbour is
    // unavailable (frame edge or another slice) or when either one's
    // reference-0 vector is already (0,0).
    let a = mv_at(&pic.mbs, None, map, -1, 0);
    let b = mv_at(&pic.mbs, None, map, 0, -1);
    let force_zero = |n: Option<([i16; 2], i32)>| match n {
        None => true,
        Some((mv, r)) => r == 0 && mv == [0, 0],
    };
    let mvp = if force_zero(a) || force_zero(b) {
        [0i16; 2]
    } else {
        mvp_l0_parts(&pic.mbs, None, map, 0, 0, 4, 4, 0)
    };

    {
        let m = &mut pic.mbs[idx];
        m.mb_type = MbType::PSkip;
        m.skip = true;
        m.cbp = 0;
        m.slice_id = cx.slice_id;
        m.dbg_idx = idx as u32;
        m.qp_y = cx.qp_prev.clamp(0, 51) as u8;
        m.disable_deblock_idc = cx.h.disable_deblock_idc;
        m.filter_offset_a = cx.h.offset_a;
        m.filter_offset_b = cx.h.offset_b;
        for b in m.mv.iter_mut() {
            *b = [mvp[0], mvp[1]];
        }
        for r in m.ref_idx.iter_mut() {
            *r = 0;
        }
        m.mv_valid = [true; 16];
        m.nz = [0; 24];
    }
    mc_partition(
        &mut pic.buf,
        cx,
        dpb,
        map.x * 16,
        map.y * 16,
        16,
        16,
        [i32::from(mvp[0]), i32::from(mvp[1])],
        0,
    )?;
    Ok(())
}

/// B_Skip / B_Direct_16x16 reconstruction: temporal or spatial direct
/// prediction per 8x8 region, no residual, no coded motion.
fn b_skip(pic: &mut Pic, cx: &mut SliceCx<'_>, dpb: &Dpb, idx: usize, wm: usize) -> Result<()> {
    {
        let m = &mut pic.mbs[idx];
        m.mb_type = MbType::BSkip;
        m.slice_id = cx.slice_id;
        m.dbg_idx = idx as u32;
        m.qp_y = cx.qp_prev.clamp(0, 51) as u8;
        m.disable_deblock_idc = cx.h.disable_deblock_idc;
        m.filter_offset_a = cx.h.offset_a;
        m.filter_offset_b = cx.h.offset_b;
        m.nz = [0; 24];
        m.skip = true;
        cx.qp_delta_prev = 0;
        // Reference `decode_mb_skip`: a skipped MB has no residual and
        // resets `last_qscale_diff` — the next coded MB's qp_delta
        // context must see prev=0.
        cx.qp_delta_prev = 0;
        m.direct_mask = 0b1111;
        m.direct_spatial = cx.h.direct_spatial;
    }
    // Borrow-split: the MB state comes out of pic.mbs for the call.
    let mut m = pic.mbs[idx].clone();
    let r = direct_motion(pic, cx, dpb, idx, wm, &mut m, 0b1111);

    pic.mbs[idx] = m;
    r
}

/// Full macroblock decode: syntax parse then reconstruction.
fn mb_decode(
    pic: &mut Pic,
    br: &mut Br<'_>,
    cx: &mut SliceCx<'_>,
    dpb: &Dpb,
    idx: usize,
    wm: usize,
) -> Result<()> {
    let code = br.ue()?;
    let mb_type = match cx.h.slice_type {
        SliceType::I => MbType::i_slice(code)?,
        SliceType::P => MbType::p_slice(code)?,
        SliceType::B => MbType::b_slice(code)?,
    };
    let map = MbMap {
        idx,
        x: idx % wm,
        y: idx / wm,
        width: wm,
        sid: cx.slice_id,
    };
    let mut m = MbState::new();
    m.mb_type = mb_type;
    m.slice_id = cx.slice_id;
    m.dbg_idx = idx as u32;
    m.disable_deblock_idc = cx.h.disable_deblock_idc;
    m.filter_offset_a = cx.h.offset_a;
    m.filter_offset_b = cx.h.offset_b;
    m.direct_spatial = cx.h.direct_spatial;

    // ---- mb_pred / sub_mb_pred (spec 7.3.5) ----
    // Inter partitions as `(x4, y4, w4, h4, ref_idx, mvd)` on the
    // 4x4-block grid.
    let mut inter_parts: Vec<(usize, usize, usize, usize, u8, [i16; 2])> = Vec::new();
    // B-slice partitions in the shared `Part` form (per-list
    // ref+mvd); `direct_mask` marks the 8x8 groups `direct_motion`
    // reconstructs instead of the coded list.
    let mut b_parts: Vec<Part> = Vec::new();
    let mut direct_mask = 0u8;
    let mut sub_direct_allowed_8x8 = true;
    // ffmpeg `get_dct8x8_allowed` (h264_mvpred.h:157): any sub-mb
    // partitioned below 8x8 (8x4/4x8/4x4) blocks
    // transform_size_8x8_flag; B_Direct also blocks it without
    // direct_8x8_inference.
    let mut sub_blocks_8x8 = false;
    match mb_type {
        MbType::I4x4 => {
            // transform_8x8_mode_flag (spec 7.3.5 mb_pred) — the
            // High-profile I_8x8 syntax, gated on
            // `pps.transform_8x8_mode_flag` alone (the CABAC/ctx399
            // gate additionally fires on direct_8x8_inference).
            let t8x8_allowed = cx.pps.transform_8x8_mode;
            if t8x8_allowed && br.bit()? {
                m.transform8x8 = true;
            }
            if m.transform8x8 {
                // One mode per 8x8 group (spec 7.3.5.1
                // prev_intra8x8_pred_mode_flag); the mode fills the
                // group's four 4x4 cache cells so the 8x8 MPM reads
                // neighbours like the reference decoder.
                for blk in 0..4 {
                    let g = blk * 4;
                    let mpm = i4x4_mpm(&pic.mbs, &m, map, g);
                    let flag = br.bit()?;
                    let mode = if flag {
                        mpm
                    } else {
                        let rem = br.bits(3)? as u8;
                        if rem < mpm { rem } else { rem + 1 }
                    };
                    for i in 0..4 {
                        let (bx, by) = block_xy(g + i);
                        m.i4x4_modes[by * 4 + bx] = mode;
                    }
                }
            } else {
                // Parsed in luma4x4BlkIdx (group-major) order; stored
                // raster-indexed for neighbour lookup.
                for blk in 0..16 {
                    let (bx, by) = block_xy(blk);
                    let r = by * 4 + bx;
                    let mpm = i4x4_mpm(&pic.mbs, &m, map, blk);
                    let flag = br.bit()?;
                    let mode = if flag {
                        mpm
                    } else {
                        let rem = br.bits(3)? as u8;
                        if rem < mpm { rem } else { rem + 1 }
                    };
                    let _ = (blk, r);
                    m.i4x4_modes[r] = mode;
                }
            }
            m.chroma_pred = br.ue()? as u8;
            if m.chroma_pred > 3 {
                return Err(Error::BadValue("intra_chroma_pred_mode over 3"));
            }
        }
        MbType::I16x16 { .. } => {
            m.chroma_pred = br.ue()? as u8;
            if m.chroma_pred > 3 {
                return Err(Error::BadValue("intra_chroma_pred_mode over 3"));
            }
        }
        MbType::IPcm => {}
        MbType::PSkip => return Err(Error::BadValue("P_Skip inside mb_skip_run gap")),
        MbType::P8x8 | MbType::P8x8Ref0 => {
            let mut sub_types = [SubMbType::S8x8; 4];
            for st in sub_types.iter_mut() {
                *st = SubMbType::from_code(br.ue()?)?;
            }
            // ffmpeg ff_h264_p_sub_mb_type_info (h264data.c): sub code
            // 3 (4x4 partitions) maps to MB_TYPE_8x8 which *blocks*
            // the flag — the flag bit exists only when every sub is
            // an 8x8 partition.
            if sub_types.iter().any(|st| !matches!(st, SubMbType::S8x8)) {
                sub_blocks_8x8 = true;
            }
            // ref_idx_l0 coded once per 8x8 sub-mb (spec 7.3.5.1);
            // P_8x8ref0 implies zero.
            let mut refs = [0u8; 4];
            let need_ref = cx.h.num_ref_idx_l0_active > 1 && mb_type != MbType::P8x8Ref0;
            for r in refs.iter_mut() {
                if need_ref {
                    *r = br.te(cx.h.num_ref_idx_l0_active - 1)? as u8;
                    if (*r as usize) >= (cx.h.num_ref_idx_l0_active as usize) {
                        return Err(Error::BadValue("ref_idx_l0 out of list"));
                    }
                }
            }
            for (sm, &st) in sub_types.iter().enumerate() {
                let (sizes, nparts) = st.parts();
                let sx = (sm % 2) * 2;
                let sy = (sm / 2) * 2;
                for pi in 0..nparts {
                    let _ = sizes.get(pi);
                    let mvd = [br.se()?, br.se()?];
                    let (x4, y4, w4, h4) = sub_part_geo(sx, sy, st, pi);
                    inter_parts.push((x4, y4, w4, h4, refs[sm], clamp_mvd(mvd)));
                }
            }
        }
        MbType::BDirect => {
            // B_Direct_16x16: no coded motion; `direct_motion`
            // derives refs + MVs for all four 8x8 groups.
            direct_mask = 0b1111;
        }
        MbType::B16x16 { dirs } => {
            // Spec 7.3.5.1: for every list, ref_idx then mvd (the
            // single 16x16 partition). Read order: all L0 fields,
            // then all L1 fields.
            let mut p = Part::new(0, 0, 4, 4, dirs);
            if cx.h.num_ref_idx_l0_active > 1 && dirs & 1 != 0 {
                p.ref0 = br.te(cx.h.num_ref_idx_l0_active - 1)? as u8;
                if p.ref0 as usize >= cx.h.num_ref_idx_l0_active as usize {
                    return Err(Error::BadValue("ref_idx_l0 out of list"));
                }
            }
            if cx.h.num_ref_idx_l1_active > 1 && dirs & 2 != 0 {
                p.ref1 = br.te(cx.h.num_ref_idx_l1_active - 1)? as u8;
                if p.ref1 as usize >= cx.h.num_ref_idx_l1_active as usize {
                    return Err(Error::BadValue("ref_idx_l1 out of list"));
                }
            }
            if dirs & 1 != 0 {
                p.mvd0 = clamp_mvd([br.se()?, br.se()?]);
            }
            if dirs & 2 != 0 {
                p.mvd1 = clamp_mvd([br.se()?, br.se()?]);
            }
            b_parts.push(p);
        }
        MbType::BPart { p0, p1 } => {
            let mut ps = [Part::new(0, 0, p0.0 as usize, p0.1 as usize, p0.2), {
                let g = part_geo_b(0, p0.0, p0.1);
                Part::new(g.0, g.1, g.2, g.3, p1.2)
            }];
            // All L0 refs, all L1 refs, all L0 mvds, all L1 mvds.
            for p in ps.iter_mut() {
                if cx.h.num_ref_idx_l0_active > 1 && p.dirs & 1 != 0 {
                    p.ref0 = br.te(cx.h.num_ref_idx_l0_active - 1)? as u8;
                    if p.ref0 as usize >= cx.h.num_ref_idx_l0_active as usize {
                        return Err(Error::BadValue("ref_idx_l0 out of list"));
                    }
                }
            }
            for p in ps.iter_mut() {
                if cx.h.num_ref_idx_l1_active > 1 && p.dirs & 2 != 0 {
                    p.ref1 = br.te(cx.h.num_ref_idx_l1_active - 1)? as u8;
                    if p.ref1 as usize >= cx.h.num_ref_idx_l1_active as usize {
                        return Err(Error::BadValue("ref_idx_l1 out of list"));
                    }
                }
            }
            for p in ps.iter_mut() {
                if p.dirs & 1 != 0 {
                    p.mvd0 = clamp_mvd([br.se()?, br.se()?]);
                }
            }
            for p in ps.iter_mut() {
                if p.dirs & 2 != 0 {
                    p.mvd1 = clamp_mvd([br.se()?, br.se()?]);
                }
            }
            b_parts.extend_from_slice(&ps);
        }
        MbType::B8x8 => {
            let mut subs = [SubMbType::BDirect; 4];
            for st in subs.iter_mut() {
                let code = br.ue()?;
                *st = SubMbType::from_code_b(code)?;
            }
            for st in &subs {
                match st {
                    SubMbType::BDirect => sub_direct_allowed_8x8 = false,
                    SubMbType::BInter { w4, h4, .. } if !(w4 == &2 && h4 == &2) => {
                        sub_blocks_8x8 = true;
                    }
                    _ => {}
                }
            }
            // Per-list ref reads: every L0-bearing sub first, then L1
            // (spec 7.3.5.1 loops lists outer, sub-mbs inner).
            let mut refs0 = [0u8; 4];
            let mut refs1 = [0u8; 4];
            if cx.h.num_ref_idx_l0_active > 1 {
                for (sm, st) in subs.iter().enumerate() {
                    let uses = matches!(st, SubMbType::BInter { dirs, .. } if dirs & 1 != 0);
                    if uses {
                        refs0[sm] = br.te(cx.h.num_ref_idx_l0_active - 1)? as u8;
                        if refs0[sm] as usize >= cx.h.num_ref_idx_l0_active as usize {
                            return Err(Error::BadValue("ref_idx_l0 out of list"));
                        }
                    }
                }
            }
            if cx.h.num_ref_idx_l1_active > 1 {
                for (sm, st) in subs.iter().enumerate() {
                    let uses = matches!(st, SubMbType::BInter { dirs, .. } if dirs & 2 != 0);
                    if uses {
                        refs1[sm] = br.te(cx.h.num_ref_idx_l1_active - 1)? as u8;
                        if refs1[sm] as usize >= cx.h.num_ref_idx_l1_active as usize {
                            return Err(Error::BadValue("ref_idx_l1 out of list"));
                        }
                    }
                }
            }
            // Per-list mvd reads (all L0 mvds, then all L1 mvds).
            for l in 0..2usize {
                for (sm, &st) in subs.iter().enumerate() {
                    let sx = (sm % 2) * 2;
                    let sy = (sm / 2) * 2;
                    if let SubMbType::BInter {
                        w4,
                        h4,
                        nparts,
                        dirs,
                    } = st
                    {
                        if dirs & (1 << l) == 0 {
                            continue;
                        }
                        for pi in 0..nparts as usize {
                            let mvd = clamp_mvd([br.se()?, br.se()?]);
                            let (px, py) = b_part_xy(st, pi);
                            // Find or create the Part for this
                            // sub-partition: mvd lists read L0 first,
                            // so parts appear during l == 0.
                            let x4 = sx + px;
                            let y4 = sy + py;
                            if l == 0 {
                                let mut p = Part::new(x4, y4, w4 as usize, h4 as usize, dirs);
                                p.ref0 = refs0[sm];
                                p.ref1 = refs1[sm];
                                p.mvd0 = mvd;
                                b_parts.push(p);
                            } else if let Some(p) = b_parts.iter_mut().find(|p| {
                                p.x4 == x4
                                    && p.y4 == y4
                                    && p.w4 == w4 as usize
                                    && p.h4 == h4 as usize
                            }) {
                                p.mvd1 = mvd;
                            }
                        }
                    }
                }
            }
            for (sm, st) in subs.iter().enumerate() {
                if *st == SubMbType::BDirect {
                    direct_mask |= 1 << sm;
                }
            }
        }
        _ => {
            // P16x16 / P16x8 / P8x16.
            let parts: [(usize, usize, usize, usize); 2] = match mb_type {
                MbType::P16x16 => [(0, 0, 4, 4), (0, 0, 0, 0)],
                MbType::P16x8 => [(0, 0, 4, 2), (0, 2, 4, 2)],
                MbType::P8x16 => [(0, 0, 2, 4), (2, 0, 2, 4)],
                _ => unreachable!(),
            };
            let nparts = if mb_type == MbType::P16x16 { 1 } else { 2 };
            let need_ref = cx.h.num_ref_idx_l0_active > 1;
            // Spec 7.3.5.1 order: ref_idx_l0 for EVERY partition first,
            // then mvd_l0 for every partition. Interleaving them (as
            // ref,mvd,ref,mvd) desyncs the stream whenever a partition
            // uses a non-zero reference index.
            let mut refs = [0u8; 2];
            for r in refs.iter_mut().take(nparts) {
                if need_ref {
                    *r = br.te(cx.h.num_ref_idx_l0_active - 1)? as u8;
                    if *r as usize >= cx.h.num_ref_idx_l0_active as usize {
                        return Err(Error::BadValue("ref_idx_l0 out of list"));
                    }
                }
            }
            for (pi, &(x4, y4, w4, h4)) in parts.iter().take(nparts).enumerate() {
                let mvd = clamp_mvd([br.se()?, br.se()?]);
                inter_parts.push((x4, y4, w4, h4, refs[pi], mvd));
            }
        }
    }

    // ---- coded_block_pattern + mb_qp_delta (spec 7.3.5) ----
    // B_Direct_16x16 codes cbp like any other inter MB (see the
    // CABAC twin of this block).
    let (mut cbp_luma, mut cbp_chroma) = (0u8, 0u8);
    if mb_type != MbType::IPcm {
        if let MbType::I16x16 {
            cbp_chroma: cc,
            cbp_luma: cl,
            ..
        } = mb_type
        {
            cbp_luma = cl;
            cbp_chroma = cc;
        } else {
            let cbp = br.ue()? as usize;
            if cbp >= 48 {
                return Err(Error::BadValue("coded_block_pattern over 47"));
            }
            let v = if mb_type.is_intra() {
                CBP_INTRA[cbp]
            } else {
                CBP_INTER[cbp]
            };
            cbp_luma = v % 16;
            cbp_chroma = v / 16;
        }
    }
    // transform_size_8x8_flag (ffmpeg h264_cabac.c:2347:
    // `dct8x8_allowed && (cbp&15) && !IS_INTRA`; dct8x8_allowed is the
    // PPS flag narrowed by get_dct8x8_allowed for P_8x8/B_8x8).
    let dct8x8_allowed = cx.pps.transform_8x8_mode
        && !matches!(mb_type, MbType::I16x16 { .. } | MbType::IPcm)
        && !sub_blocks_8x8
        && (sub_direct_allowed_8x8 || cx.sps.direct_8x8_inference)
        && (!matches!(mb_type, MbType::BDirect) || cx.sps.direct_8x8_inference);
    let t8x8 = dct8x8_allowed && cbp_luma > 0 && !mb_type.is_intra() && br.bit()?;
    m.direct_mask = direct_mask;
    // Intra keeps the I_8x8 flag read during mb_pred (ffmpeg
    // h264_cavlc.c:797); inter takes the read here.
    m.transform8x8 = if mb_type.is_intra() {
        m.transform8x8
    } else {
        t8x8
    };
    let coded = cbp_luma > 0 || cbp_chroma > 0 || matches!(mb_type, MbType::I16x16 { .. });
    let mut qp_y = cx.qp_prev;
    if coded && mb_type != MbType::IPcm {
        let delta = br.se()?;
        qp_y = (i64::from(cx.qp_prev) + i64::from(delta)).rem_euclid(52) as i32;
        cx.qp_prev = qp_y;
    }
    if mb_type == MbType::IPcm {
        qp_y = 0;
        cx.qp_prev = 0;
    }
    m.qp_y = qp_y.clamp(0, 51) as u8;

    // ---- residual parse (spec 7.3.5.3) ----
    // `nz_acc` is the in-flight nC context: neighbour reads come from
    // `pic.mbs` (fully decoded MBs) and same-MB reads from `nz_acc`.
    let mut nz_acc = [0u8; 24];
    let mut luma_res = [[0i32; 16]; 16];
    let mut luma_res8 = [[0i32; 64]; 4];
    let mut dc_y = [0i32; 16];
    let mut chroma_ac = [[[0i32; 16]; 4]; 2];
    let mut chroma_dc = [[0i32; 4]; 2];
    if mb_type != MbType::IPcm {
        if let MbType::I16x16 { .. } = mb_type {
            let nc = nc_value(&pic.mbs, &nz_acc, map, 0, Plane::Luma);
            let r = cavlc::decode_block(br, nc, BlockKind::LumaOrChromaAc4x4)?;
            for s in 0..16 {
                // The 4x4 DC grid is raster-ordered over luma blocks:
                // position = by*4 + bx of the block (spec 8.5.11.1).
                dc_y[cavlc::raster_index(BlockKind::LumaOrChromaAc4x4, s)] = r.levels[s];
            }
            for g in 0..4usize {
                if cbp_luma & (1 << g) == 0 {
                    continue;
                }
                for s4 in 0..4usize {
                    let blk = g * 4 + s4;
                    let nc = nc_value(&pic.mbs, &nz_acc, map, blk, Plane::Luma);
                    let r = cavlc::decode_block(br, nc, BlockKind::Ac15)?;
                    for s in 0..15 {
                        let ri = cavlc::raster_index(BlockKind::Ac15, s);
                        luma_res[blk][ri] = r.levels[s];
                    }
                    nz_acc[blk] = r.total_coeff;
                }
            }
        } else if cbp_luma > 0 {
            for (g, res8) in luma_res8.iter_mut().enumerate() {
                if cbp_luma & (1 << g) == 0 {
                    continue;
                }
                if m.transform8x8 {
                    // CAVLC 8x8: four 16-coefficient reads per group
                    // (spec 7.3.5.3.2), each scattered into the 8x8
                    // coefficient grid by `CAVLC_SCAN_8X8` — the same
                    // `zigzag_scan8x8_cavlc` the reference decoder uses.
                    for s4 in 0..4usize {
                        let blk = g * 4 + s4;
                        let nc = nc_value(&pic.mbs, &nz_acc, map, blk, Plane::Luma);
                        let r = cavlc::decode_block(br, nc, BlockKind::LumaOrChromaAc4x4)?;
                        for k in 0..16 {
                            let ri = CAVLC_SCAN_8X8[16 * s4 + k] as usize;
                            res8[ri] = r.levels[k];
                        }
                        nz_acc[blk] = r.total_coeff;
                    }
                } else {
                    for s4 in 0..4usize {
                        let blk = g * 4 + s4;
                        let nc = nc_value(&pic.mbs, &nz_acc, map, blk, Plane::Luma);
                        let r = cavlc::decode_block(br, nc, BlockKind::LumaOrChromaAc4x4)?;
                        for s in 0..16 {
                            let ri = cavlc::raster_index(BlockKind::LumaOrChromaAc4x4, s);
                            luma_res[blk][ri] = r.levels[s];
                        }
                        nz_acc[blk] = r.total_coeff;
                    }
                }
            }
        }
        if cbp_chroma > 0 {
            for dc in chroma_dc.iter_mut() {
                let r = cavlc::decode_block(br, -1, BlockKind::ChromaDc)?;
                for s in 0..4 {
                    dc[cavlc::raster_index(BlockKind::ChromaDc, s)] = r.levels[s];
                }
            }
            if cbp_chroma == 2 {
                for c in 0..2usize {
                    for b in 0..4usize {
                        let nc = nc_value(&pic.mbs, &nz_acc, map, b, Plane::chroma(c));
                        let r = cavlc::decode_block(br, nc, BlockKind::Ac15)?;
                        for s in 0..15 {
                            let ri = cavlc::raster_index(BlockKind::Ac15, s);
                            chroma_ac[c][b][ri] = r.levels[s];
                        }
                        nz_acc[16 + c * 4 + b] = r.total_coeff;
                    }
                }
            }
        }
    }
    m.nz = nz_acc;

    // ---- reconstruction ----
    let px0 = map.x * 16;
    let py0 = map.y * 16;
    match mb_type {
        MbType::IPcm => {
            br.byte_align();
            for row in 0..16 {
                for x in 0..16 {
                    pic.buf.y[(py0 + row) * pic.buf.w + px0 + x] = br.byte()?;
                }
            }
            for row in 0..8 {
                for x in 0..8 {
                    let o = (py0 / 2 + row) * (pic.buf.w / 2) + px0 / 2 + x;
                    pic.buf.cb[o] = br.byte()?;
                    pic.buf.cr[o] = br.byte()?;
                }
            }
            m.nz = [16; 24];
        }
        MbType::I4x4 => {
            if m.transform8x8 {
                // I_8x8: predict one 8x8 group at a time in raster
                // order — each group reads the reconstructed samples
                // of the earlier groups (spec 8.3.5.2 availability).
                for (g, raw_resid) in luma_res8.iter().enumerate() {
                    let nb = gather_luma8x8(&pic.buf, &pic.mbs, map, g, cx.pps);
                    // Top-left 4x4 cell of this 8x8 group on the raster
                    // mode grid: y*4 + x with (x, y) = (2*(g%2), 2*(g/2)).
                    let mode = m.i4x4_modes[8 * (g / 2) + 2 * (g % 2)];
                    let mut pred = [0u8; 64];
                    intra::pred8x8(mode, &nb, &mut pred);

                    let mut resid = *raw_resid;
                    if coded && cbp_luma & (1 << g) != 0 {
                        transform::dequant_8x8(&mut resid, m.qp_y);
                        transform::inverse_8x8(&mut resid);
                    }
                    let (bx, by) = ((g % 2) * 8, (g / 2) * 8);
                    for yy in 0..8 {
                        for xx in 0..8 {
                            let v = i32::from(pred[yy * 8 + xx]) + resid[yy * 8 + xx];
                            pic.buf.y[(py0 + by + yy) * pic.buf.w + px0 + bx + xx] =
                                v.clamp(0, 255) as u8;
                        }
                    }
                }
            } else {
                // Reconstruct in group-major coding order.
                for (blk, raw_resid) in luma_res.iter().enumerate() {
                    let (bx, by) = block_xy(blk);
                    let r = by * 4 + bx;
                    let nb = gather4x4(&pic.buf, &pic.mbs, map, bx, by, blk, cx.pps);
                    let mut pred = [0u8; 16];
                    intra::pred4x4(m.i4x4_modes[r], &nb, &mut pred)?;
                    let mut resid = *raw_resid;
                    if coded {
                        transform::dequant_4x4(&mut resid, m.qp_y);
                        transform::inverse_4x4(&mut resid)?;
                    }
                    for yy in 0..4 {
                        for xx in 0..4 {
                            let v = i32::from(pred[yy * 4 + xx]) + resid[yy * 4 + xx];
                            pic.buf.y[(py0 + by * 4 + yy) * pic.buf.w + px0 + bx * 4 + xx] =
                                v.clamp(0, 255) as u8;
                        }
                    }
                }
            }
            chroma_recon(
                pic, cx, map, &m, &chroma_ac, &chroma_dc, cbp_chroma, px0, py0,
            )?;
        }
        MbType::I16x16 { pred, .. } => {
            let nb = gather16x16(&pic.buf, &pic.mbs, map, cx.pps);
            let mut out = [0u8; 256];
            intra::pred16x16(pred, &nb, &mut out)?;
            let mut dc = dc_y;
            transform::inv_luma_dc(&mut dc, m.qp_y);
            for (blk, raw_resid) in luma_res.iter().enumerate() {
                let (bx, by) = block_xy(blk);
                let mut resid = *raw_resid;
                resid[0] = dc[by * 4 + bx];
                // AC positions dequantise only when the 8x8 group was
                // coded; the Hadamard-scaled DC in slot 0 plus zeros
                // still goes through the inverse transform either way.
                if cbp_luma & (1 << (blk / 4)) != 0 {
                    for (i, v) in resid.iter_mut().enumerate().skip(1) {
                        let (x, y) = (i % 4, i / 4);
                        *v = transform::dequant_coeff(*v, m.qp_y, x, y);
                    }
                }
                transform::inverse_4x4(&mut resid)?;
                for yy in 0..4 {
                    for xx in 0..4 {
                        let v =
                            i32::from(out[(by * 4 + yy) * 16 + bx * 4 + xx]) + resid[yy * 4 + xx];
                        pic.buf.y[(py0 + by * 4 + yy) * pic.buf.w + px0 + bx * 4 + xx] =
                            v.clamp(0, 255) as u8;
                    }
                }
            }
            chroma_recon(
                pic, cx, map, &m, &chroma_ac, &chroma_dc, cbp_chroma, px0, py0,
            )?;
        }
        _ => {
            // Direct groups own their motion + MC first (spec 8.4.1.2).
            if direct_mask != 0 {
                let mut dm = m.clone();
                dm.direct_mask = direct_mask;
                let r = direct_motion(pic, cx, dpb, idx, wm, &mut dm, direct_mask);
                m = dm;
                r?;
            }
            // Inter: per partition MVP + MC, then residual add.
            for &(x4, y4, w4, h4, ref_idx, mvd) in &inter_parts {
                let mvp = mvp_lx_parts(&pic.mbs, Some(&m), map, x4, y4, w4, h4, ref_idx, 0);
                let mv = [
                    (i32::from(mvp[0]) + i32::from(mvd[0])).clamp(-8192, 8191) as i16,
                    (i32::from(mvp[1]) + i32::from(mvd[1])).clamp(-8192, 8191) as i16,
                ];
                for yy in y4..y4 + h4 {
                    for xx in x4..x4 + w4 {
                        let b = block_index(xx, yy);
                        m.mv[b] = mv;
                        m.ref_idx[b] = ref_idx;
                        m.mv_valid[b] = true;
                    }
                }
                mc_partition(
                    &mut pic.buf,
                    cx,
                    dpb,
                    px0 + x4 * 4,
                    py0 + y4 * 4,
                    w4 * 4,
                    h4 * 4,
                    [i32::from(mv[0]), i32::from(mv[1])],
                    ref_idx,
                )?;
            }
            for p in &b_parts {
                reconstruct_part(pic, cx, dpb, map, &mut m, *p)?;
            }
            // Add luma residual where cbp says coefficients exist.

            if m.transform8x8 {
                for (g, raw_resid) in luma_res8.iter().enumerate() {
                    if raw_resid.iter().all(|&v| v == 0) {
                        continue;
                    }
                    let (bx, by) = ((g % 2) * 8, (g / 2) * 8);
                    let mut resid = *raw_resid;
                    transform::dequant_8x8(&mut resid, m.qp_y);

                    transform::inverse_8x8(&mut resid);
                    let o = (py0 + by) * pic.buf.w + px0 + bx;
                    transform::add_residual_8x8(&mut pic.buf.y[o..], pic.buf.w, &resid);
                }
            } else {
                for (blk, raw_resid) in luma_res.iter().enumerate() {
                    if raw_resid.iter().all(|&v| v == 0) {
                        continue;
                    }
                    let (bx, by) = block_xy(blk);
                    let mut resid = *raw_resid;
                    transform::dequant_4x4(&mut resid, m.qp_y);

                    transform::inverse_4x4(&mut resid)?;
                    let o = (py0 + by * 4) * pic.buf.w + px0 + bx * 4;
                    transform::add_residual_4x4(&mut pic.buf.y[o..], pic.buf.w, &resid);
                }
            }
            chroma_recon(
                pic, cx, map, &m, &chroma_ac, &chroma_dc, cbp_chroma, px0, py0,
            )?;
        }
    }

    pic.mbs[idx] = m;
    Ok(())
}

/// `mvd` clamped to the representable quarter-pel range (a hostile
/// `se()` can produce values far beyond i16).
fn clamp_mvd(mvd: [i32; 2]) -> [i16; 2] {
    [
        mvd[0].clamp(-8192, 8191) as i16,
        mvd[1].clamp(-8192, 8191) as i16,
    ]
}

/// Chroma reconstruction shared by all MB types: intra MBs predict
/// with `intra_chroma_pred_mode`; inter MBs keep the samples chroma MC
/// already wrote. Residual is added either way.
#[allow(clippy::too_many_arguments)]
fn chroma_recon(
    pic: &mut Pic,
    cx: &SliceCx<'_>,
    map: MbMap,
    m: &MbState,
    chroma_ac: &[[[i32; 16]; 4]; 2],
    chroma_dc: &[[i32; 4]; 2],
    cbp_chroma: u8,
    px0: usize,
    py0: usize,
) -> Result<()> {
    let cw = pic.buf.w / 2;
    for c in 0..2usize {
        let qpc = chroma_qp(cx.pps, m.qp_y, c);
        let mut pred = [0u8; 64];
        if m.mb_type.is_intra() {
            let plane: &[u8] = if c == 0 { &pic.buf.cb } else { &pic.buf.cr };
            let nb = gather8x8(plane, &pic.mbs, map, cx.pps, cw);
            intra::pred_chroma(m.chroma_pred, &nb, &mut pred)?;
        }
        let dc = transform::inv_chroma_dc(&chroma_dc[c], qpc);
        let plane = if c == 0 {
            &mut pic.buf.cb
        } else {
            &mut pic.buf.cr
        };
        for (b, (ac, &dcv)) in chroma_ac[c].iter().zip(dc.iter()).enumerate() {
            let (bx, by) = (b % 2, b / 2);
            let mut resid = *ac;
            resid[0] = dcv;
            if cbp_chroma > 0 {
                for (i, v) in resid.iter_mut().enumerate().skip(1) {
                    let (x, y) = (i % 4, i / 4);
                    *v = transform::dequant_coeff(*v, qpc, x, y);
                }
                transform::inverse_4x4(&mut resid)?;
            }
            for yy in 0..4 {
                for xx in 0..4 {
                    let o = (py0 / 2 + by * 4 + yy) * cw + px0 / 2 + bx * 4 + xx;
                    let base = if m.mb_type.is_intra() {
                        pred[(by * 4 + yy) * 8 + bx * 4 + xx]
                    } else {
                        plane[o]
                    };
                    plane[o] = (i32::from(base) + resid[yy * 4 + xx]).clamp(0, 255) as u8;
                }
            }
        }
    }
    Ok(())
}

/// Chroma QP for a plane via its own PPS offset (spec 8.5.8): Cb uses
/// `chroma_qp_index_offset`, Cr uses `second_chroma_qp_index_offset`
/// (which defaults to the first when the PPS extension tail is absent).
fn chroma_qp(pps: &Pps, qp_y: u8, c: usize) -> u8 {
    let off = if c == 0 {
        pps.chroma_qp_index_offset
    } else {
        pps.chroma_qp_index_offset_cr
    };
    let idx = (i32::from(qp_y) + off).clamp(0, 51) as usize;
    QPC_TABLE[idx]
}

/// Motion-compensates one partition's luma + chroma into `buf`
/// (weighted prediction applied when the slice carries a table).
#[allow(clippy::too_many_arguments)]
fn mc_partition(
    buf: &mut FrameBuf,
    cx: &SliceCx<'_>,
    dpb: &Dpb,
    x0: usize,
    y0: usize,
    pw: usize,
    ph: usize,
    mv: [i32; 2],
    ref_idx: u8,
) -> Result<()> {
    let ri = ref_idx as usize;
    let &ref_slot = cx
        .ref_order
        .get(ri)
        .ok_or(Error::BadValue("ref_idx past active list"))?;
    let rf = dpb
        .refs
        .get(ref_slot)
        .ok_or(Error::BadValue("ref slot missing"))?;
    let mut luma = alloc::vec![0u8; pw * ph];
    inter::mc_luma(
        &rf.y,
        buf.w,
        buf.h,
        x0 as i32,
        y0 as i32,
        (mv[0], mv[1]),
        pw,
        ph,
        &mut luma,
    );

    for yy in 0..ph {
        buf.y[(y0 + yy) * buf.w + x0..(y0 + yy) * buf.w + x0 + pw]
            .copy_from_slice(&luma[yy * pw..yy * pw + pw]);
    }
    let (cx0, cy0, cw2, ch2) = (x0 / 2, y0 / 2, pw / 2, ph / 2);
    for c in 0..2usize {
        let rf_plane = if c == 0 { &rf.cb } else { &rf.cr };
        let mut tmp = alloc::vec![0u8; cw2 * ch2];
        inter::mc_chroma(
            rf_plane,
            buf.w / 2,
            buf.h / 2,
            cx0 as i32,
            cy0 as i32,
            (mv[0], mv[1]),
            cw2,
            ch2,
            &mut tmp,
        );
        if let Some(wp) = &cx.h.wp_l0 {
            if let Some(e) = wp.get(ri) {
                if e.chroma_flag {
                    inter::apply_wp(&mut tmp, e.chroma[c].0, e.chroma[c].1, cx.h.wp_denom.1);
                }
            }
        }
        let dst = if c == 0 { &mut buf.cb } else { &mut buf.cr };
        for yy in 0..ch2 {
            dst[(cy0 + yy) * (buf.w / 2) + cx0..(cy0 + yy) * (buf.w / 2) + cx0 + cw2]
                .copy_from_slice(&tmp[yy * cw2..yy * cw2 + cw2]);
        }
    }
    Ok(())
}

/// Plane selector for nC derivation.
#[derive(Copy, Clone)]
enum Plane {
    /// Luma 4x4 block (group-major idx 0..15).
    Luma,
    /// Cb chroma 4x4 AC block (0..3 on the chroma 2x2 grid).
    Cb,
    /// Cr.
    Cr,
}

impl Plane {
    fn chroma(c: usize) -> Plane {
        if c == 0 { Plane::Cb } else { Plane::Cr }
    }
    /// Index into `MbState::nz` / the in-flight accumulator.
    fn nz_idx(self, blk: usize) -> usize {
        match self {
            Plane::Luma => blk,
            Plane::Cb => 16 + blk,
            Plane::Cr => 20 + blk,
        }
    }
    /// `(x, y)` of `blk` on this plane's block grid (4x4 luma,
    /// 2x2 chroma).
    fn xy(self, blk: usize) -> (usize, usize) {
        match self {
            Plane::Luma => block_xy(blk),
            _ => (blk % 2, blk / 2),
        }
    }
    /// Back from grid coords to this plane's block index space.
    fn index(self, x: usize, y: usize) -> usize {
        match self {
            Plane::Luma => block_index(x, y),
            _ => y * 2 + x,
        }
    }
    /// Grid dimension (4 for luma, 2 for chroma).
    fn dim(self) -> i32 {
        match self {
            Plane::Luma => 4,
            _ => 2,
        }
    }
}

/// `nC` for CAVLC (spec 9.2.1): TotalCoeff of the left (nA) and top
/// (nB) blocks. Same-MB neighbours read from `nz_acc`; foreign-MB
/// neighbours read `mbs`. Unavailable neighbours drop out; both missing
/// gives nC 0 (Table 9-5 column 0).
fn nc_value(mbs: &[MbState], nz_acc: &[u8; 24], map: MbMap, blk: usize, plane: Plane) -> i32 {
    let (x, y) = plane.xy(blk);
    let dim = plane.dim();
    let fetch = |nx: i32, ny: i32| -> Option<i32> {
        if nx >= 0 && ny >= 0 && nx < dim && ny < dim {
            return Some(i32::from(
                nz_acc[plane.nz_idx(plane.index(nx as usize, ny as usize))],
            ));
        }
        // Foreign-MB block. nA (nx < 0, ny inside) comes from mbA's
        // right edge; nB (ny < 0) from mbB's bottom edge. nx<0&ny<0
        // doesn't happen here (nA/nB are axis neighbours only).
        let nidx = if nx < 0 { map.mb_a() } else { map.mb_b() }?;
        let nb = &mbs[nidx];
        if nb.slice_id != map.sid {
            return None;
        }
        if nb.mb_type == MbType::IPcm {
            return Some(16);
        }
        let lx = (nx + dim) % dim;
        let ly = (ny + dim) % dim;
        let v = i32::from(nb.nz[plane.nz_idx(plane.index(lx as usize, ly as usize))]);
        Some(v)
    };
    let na = fetch(x as i32 - 1, y as i32);
    let nb = fetch(x as i32, y as i32 - 1);
    match (na, nb) {
        (Some(a), Some(b)) => (a + b + 1) >> 1,
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 0,
    }
}

/// Spec 8.3.1.1's `predIntra4x4PredMode` for *group-major* block `blk`:
/// `min(modeA, modeB)` when both neighbours are available, DC when
/// either is not; a coded but non-I4x4 neighbour contributes DC.
fn i4x4_mpm(mbs: &[MbState], cur: &MbState, map: MbMap, blk: usize) -> u8 {
    let [a, b, _c, _d] = intra4x4_neighbours(blk);
    // Spec 8.3.1.1: a neighbour is *unavailable* when its block is
    // outside the picture, in a not-yet-decoded MB, a not-yet-coded
    // same-MB block, or — under constrained_intra_pred — an inter MB.
    // When EITHER A or B is unavailable the most-probable mode is DC
    // outright (dcPredModePredictedFlag is false); only when both are
    // available is it min(modeA, modeB), where a non-I4x4 neighbour
    // contributes DC.
    let mode = |nb: Nb, x: usize, y: usize| -> Option<u8> {
        match nb {
            Nb::Curr => {
                if block_index(x, y) < blk && cur.i4x4_modes[y * 4 + x] != 0xff {
                    Some(cur.i4x4_modes[y * 4 + x])
                } else {
                    None
                }
            }
            Nb::None => None,
            other => {
                let nidx = match other {
                    Nb::A => map.mb_a(),
                    Nb::B => map.mb_b(),
                    Nb::C => map.mb_c(),
                    Nb::D => map.mb_d(),
                    _ => None,
                };
                match nidx {
                    Some(i) => {
                        let nb2 = &mbs[i];
                        if nb2.slice_id != map.sid {
                            None
                        } else if nb2.mb_type == MbType::I4x4 {
                            Some(nb2.i4x4_modes[y * 4 + x])
                        } else {
                            Some(2)
                        }
                    }
                    None => None,
                }
            }
        }
    };
    let ma = mode(a.0, a.1, a.2);
    let mb_ = mode(b.0, b.1, b.2);
    match (ma, mb_) {
        (Some(x), Some(y)) => x.min(y),
        _ => 2,
    }
}

/// MVP for one partition (spec 8.4.1.3): directional special cases
/// then the A/B/C median. `cur` carries this MB's in-flight state —
/// same-MB partitions decoded earlier in this MB are valid neighbours.
#[allow(clippy::too_many_arguments)]
fn mvp_l0_parts(
    mbs: &[MbState],
    cur: Option<&MbState>,
    map: MbMap,
    x4: usize,
    y4: usize,
    w4: usize,
    h4: usize,
    ref_idx: u8,
) -> [i16; 2] {
    // Neighbour blocks (spec 6.4.11.7 / 8.4.1.3.2):
    //   A: the 4x4 block left of (x4, y4)
    //   B: above (x4, y4)
    //   C: above (x4 + w4, y4) — falls back to D when unavailable.
    let a = mv_at(mbs, cur, map, x4 as i32 - 1, y4 as i32);
    let b = mv_at(mbs, cur, map, x4 as i32, y4 as i32 - 1);
    let mut c = mv_at(mbs, cur, map, x4 as i32 + w4 as i32, y4 as i32 - 1);
    if c.is_none() {
        c = mv_at(mbs, cur, map, x4 as i32 - 1, y4 as i32 - 1);
    }
    let (mv_a, ref_a) = a.unwrap_or(([0, 0], -1));
    let (mv_b, ref_b) = b.unwrap_or(([0, 0], -1));
    let (mv_c, ref_c) = c.unwrap_or(([0, 0], -1));
    let ri = i32::from(ref_idx);

    // Directional single-candidate rules are MB-partition-only (spec
    // 8.4.1.1 "If MbPartWidth is 16 and MbPartHeight is 8..." / the
    // 8x16 analogue; ffmpeg keeps them in pred_16x8/pred_8x16_motion,
    // called only from MB-partition decode). Sub-mb partitions
    // (8x8/8x4/4x8/4x4) always take the median path — the previous
    // 8x4/4x8 cases generalised the rules down to sub-mbs and broke
    // every mixed-sub P8x8 MB in t2 (t2_8x8_part frame 1+).
    if w4 == 4 && h4 == 2 {
        if y4 == 0 && ref_b == ri {
            return mv_b;
        }
        if y4 == 2 && ref_a == ri {
            return mv_a;
        }
    }
    if w4 == 2 && h4 == 4 {
        if x4 == 0 && ref_a == ri {
            return mv_a;
        }
        if x4 == 2 && ref_c == ri {
            return mv_c;
        }
    }
    // Median (spec 8-211/8-212): a single matching ref short-circuits;
    // with B and C both unavailable the predictor is A alone.
    if b.is_none() && c.is_none() {
        return mv_a;
    }
    let matches = [ref_a == ri, ref_b == ri, ref_c == ri];
    if matches.iter().filter(|&&m| m).count() == 1 {
        if matches[0] {
            return mv_a;
        }
        if matches[1] {
            return mv_b;
        }
        return mv_c;
    }
    [
        med3(mv_a[0], mv_b[0], mv_c[0]),
        med3(mv_a[1], mv_b[1], mv_c[1]),
    ]
}

fn med3(a: i16, b: i16, c: i16) -> i16 {
    a.max(b).min(a.min(b).max(c))
}

/// MV + ref of the inter block containing 4x4-grid sample `(x, y)` —
/// negative or ≥4 coordinates reach into the neighbour MBs. Returns
/// `Some((mv, ref))` where `ref == -1` means "cannot predict from it"
/// (intra-coded or undecoded); `None` when the position is outside the
/// picture.
fn mv_at(
    mbs: &[MbState],
    cur: Option<&MbState>,
    map: MbMap,
    x: i32,
    y: i32,
) -> Option<([i16; 2], i32)> {
    if (0..4).contains(&x) && (0..4).contains(&y) {
        let nb = cur?;
        let b = block_index(x as usize, y as usize);
        if nb.mb_type.is_intra() {
            return Some(([0, 0], -1));
        }
        let r = nb.ref_idx[b];
        // A same-MB block whose partition has not been decoded yet
        // (ref 0xff or MV not committed — refs commit before mvds in
        // P8x8) is an unavailable neighbour (spec 8.4.1.1), not a
        // zero-motion candidate.
        if r == 0xff || !nb.mv_valid[b] {
            return None;
        }
        return Some((nb.mv[b], i32::from(r)));
    }
    let nidx = match (x < 0, y < 0) {
        (true, false) => map.mb_a(),
        (false, true) => {
            if x >= 4 {
                map.mb_c()
            } else {
                map.mb_b()
            }
        }
        (true, true) => map.mb_d(),
        // In-MB coordinates that reach the right or below neighbour
        // are not yet coded — unavailable, not a bug.
        (false, false) => None,
    }?;
    let nb = &mbs[nidx];
    if nb.slice_id != map.sid {
        return None;
    }
    let lx = x.rem_euclid(4);
    let ly = y.rem_euclid(4);
    let b = block_index(lx as usize, ly as usize);
    if nb.mb_type.is_intra() {
        return Some(([0, 0], -1));
    }
    let r = nb.ref_idx[b];
    if !nb.mv_valid[b] {
        return Some((nb.mv[b], -1));
    }
    Some((nb.mv[b], if r == 0xff { -1 } else { i32::from(r) }))
}

/// Sub-partition geometry inside the 8x8 sub-mb whose top-left on the
/// 4x4 grid is `(sx, sy)`: returns `(x4, y4, w4, h4)` in MB coords.
fn sub_part_geo(sx: usize, sy: usize, st: SubMbType, part: usize) -> (usize, usize, usize, usize) {
    match st {
        SubMbType::S8x8 => (sx, sy, 2, 2),
        SubMbType::S8x4 => (sx, sy + part, 2, 1),
        SubMbType::S4x8 => (sx + part, sy, 1, 2),
        SubMbType::S4x4 => (sx + part % 2, sy + part / 2, 1, 1),
        // B sub-partition geometry comes from `SubMbType::parts` +
        // `b_part_xy` instead; this function only serves P types.
        SubMbType::BDirect | SubMbType::BInter { .. } => (sx, sy, 2, 2),
    }
}

/// Whether a foreign neighbour MB's samples may feed intra prediction:
/// it must be coded, and under `constrained_intra_pred` it must be
/// intra.
fn intra_ok(nb: &MbState, pps: &Pps, sid: u32) -> bool {
    nb.slice_id == sid && (!pps.constrained_intra_pred || nb.mb_type.is_intra())
}

/// Gathers prediction neighbours for a luma 4x4 block at grid `(bx, by)`
/// (group-major `blk`) into [`NbSamples`]. Availability follows spec
/// 6.4.11.4/8.3.1.1: same-MB blocks are available only when earlier in
/// coding order; foreign blocks need a coded (and under
/// `constrained_intra_pred`, intra) neighbour MB.
fn gather4x4(
    buf: &FrameBuf,
    mbs: &[MbState],
    map: MbMap,
    bx: usize,
    by: usize,
    blk: usize,
    pps: &Pps,
) -> NbSamples {
    let px = map.x * 16 + bx * 4;
    let py = map.y * 16 + by * 4;
    let mut nb = NbSamples {
        left: [0; 16],
        top: [0; 16],
        top_right: None,
        top_left: None,
        has_left: false,
        has_top: false,
    };
    let [aref, bref, cref, dref] = intra4x4_neighbours(blk);
    let foreign = |who: Nb| -> Option<&MbState> {
        let i = match who {
            Nb::A => map.mb_a(),
            Nb::B => map.mb_b(),
            Nb::C => map.mb_c(),
            Nb::D => map.mb_d(),
            _ => None,
        }?;
        mbs.get(i)
    };
    // Left: available iff its source block is usable (Curr is always
    // earlier — left-of-block is always group-major-earlier? NO: the
    // left block of (bx,by) is (bx-1,by) which is earlier; mbA is
    // always fully decoded).
    let left_ok = match aref.0 {
        Nb::Curr => true, // (bx-1, by) is always earlier
        Nb::None => false,
        w => foreign(w)
            .map(|s| intra_ok(s, pps, map.sid))
            .unwrap_or(false),
    };
    if left_ok && px > 0 {
        nb.has_left = true;
        for i in 0..4 {
            nb.left[i] = buf.y[(py + i) * buf.w + px - 1];
        }
    }
    let top_ok = match bref.0 {
        Nb::Curr => true,
        Nb::None => false,
        w => foreign(w)
            .map(|s| intra_ok(s, pps, map.sid))
            .unwrap_or(false),
    };
    if top_ok && py > 0 {
        nb.has_top = true;
        for i in 0..4 {
            nb.top[i] = buf.y[(py - 1) * buf.w + px + i];
        }
        // Top-right samples live in the C block (spec 6.4.11.4):
        // Curr(x+1,y-1) is available only if coded earlier; mbB for
        // by==0&bx<3; mbC at the top-right corner; None otherwise.
        let tr_ok = match cref.0 {
            Nb::Curr => block_index(cref.1, cref.2) < blk,
            Nb::None => false,
            w => foreign(w)
                .map(|s| intra_ok(s, pps, map.sid))
                .unwrap_or(false),
        };
        if tr_ok {
            let mut tr = [0u8; 16];
            for (i, d) in tr.iter_mut().enumerate() {
                *d = buf.y[(py - 1) * buf.w + px + 4 + i];
            }
            nb.top_right = Some(tr);
        }
    }
    // Top-left corner sample lives in the D block.
    let tl_ok = match dref.0 {
        Nb::Curr => true, // (bx-1,by-1) is always earlier
        Nb::None => false,
        w => foreign(w)
            .map(|s| intra_ok(s, pps, map.sid))
            .unwrap_or(false),
    };
    if tl_ok && px > 0 && py > 0 {
        nb.top_left = Some(buf.y[(py - 1) * buf.w + px - 1]);
    }
    nb
}

/// Neighbour samples for a 16x16 luma block (A/B/D foreign only).
fn gather16x16(buf: &FrameBuf, mbs: &[MbState], map: MbMap, pps: &Pps) -> NbSamples {
    let px = map.x * 16;
    let py = map.y * 16;
    let mut nb = NbSamples {
        left: [0; 16],
        top: [0; 16],
        top_right: None,
        top_left: None,
        has_left: false,
        has_top: false,
    };
    if let Some(a) = map.mb_a() {
        if intra_ok(&mbs[a], pps, map.sid) && px > 0 {
            nb.has_left = true;
            for i in 0..16 {
                nb.left[i] = buf.y[(py + i) * buf.w + px - 1];
            }
        }
    }
    if let Some(b) = map.mb_b() {
        if intra_ok(&mbs[b], pps, map.sid) && py > 0 {
            nb.has_top = true;
            for i in 0..16 {
                nb.top[i] = buf.y[(py - 1) * buf.w + px + i];
            }
        }
    }
    if let Some(d) = map.mb_d() {
        if intra_ok(&mbs[d], pps, map.sid) && px > 0 && py > 0 {
            nb.top_left = Some(buf.y[(py - 1) * buf.w + px - 1]);
        }
    }
    nb
}

/// Neighbour samples for an 8x8 chroma block (foreign A/B/D only).
fn gather8x8(plane: &[u8], mbs: &[MbState], map: MbMap, pps: &Pps, cw: usize) -> NbSamples {
    let px = map.x * 8;
    let py = map.y * 8;
    let mut nb = NbSamples {
        left: [0; 16],
        top: [0; 16],
        top_right: None,
        top_left: None,
        has_left: false,
        has_top: false,
    };
    if let Some(a) = map.mb_a() {
        if intra_ok(&mbs[a], pps, map.sid) && px > 0 {
            nb.has_left = true;
            for i in 0..8 {
                nb.left[i] = plane[(py + i) * cw + px - 1];
            }
        }
    }
    if let Some(b) = map.mb_b() {
        if intra_ok(&mbs[b], pps, map.sid) && py > 0 {
            nb.has_top = true;
            for i in 0..8 {
                nb.top[i] = plane[(py - 1) * cw + px + i];
            }
        }
    }
    if let Some(d) = map.mb_d() {
        if intra_ok(&mbs[d], pps, map.sid) && px > 0 && py > 0 {
            nb.top_left = Some(plane[(py - 1) * cw + px - 1]);
        }
    }
    nb
}

#[cfg(test)]
mod trace_tests {
    extern crate std;
    use std::fs;

    #[test]
    fn trace_t3() {
        let s = fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/t1_32x32_ip.h264"
        ))
        .unwrap();
        crate::decode(&s).unwrap();
    }
}

// ======================== B slices ========================

/// Implicit weighted-prediction table `w0` for one (ref0, ref1) pair
/// (spec 8.4.3.3.1 + reference-decoder guard): 64 − DistScaleFactor
/// when the factor lands in [-64, 128], else the default 32.
fn implicit_w0(cur_poc: i64, poc0: i64, poc1: i64) -> i32 {
    let td = (poc1 - poc0).clamp(-128, 127);
    if td == 0 {
        return 32;
    }
    let tb = (cur_poc - poc0).clamp(-128, 127);
    let tx = (16384 + (td.abs() >> 1)) / td;
    let dsf = (tb * tx + 32) >> 8;
    if (-64..=128).contains(&dsf) {
        (64 - dsf) as i32
    } else {
        32
    }
}

/// `DistScaleFactor` used by temporal direct (spec 8.4.1.2.3): the
/// reference decoder's `get_scale_factor` — clip-1024 result domain.
fn direct_dsf(cur_poc: i64, poc0: i64, poc1: i64) -> i32 {
    let td = (poc1 - poc0).clamp(-128, 127);
    if td == 0 {
        return 256;
    }
    let tb = (cur_poc - poc0).clamp(-128, 127);
    let tx = (16384 + (td.abs() >> 1)) / td;
    ((tb * tx + 32) >> 6).clamp(-512, 511) as i32
}

/// Maps a ref index on the colocated picture's list `l` back onto the
/// current RefPicList0 via `frame_num` identity (the reference
/// decoder's `map_col_to_list0`). Unmapped entries read as 0.
fn col_ref_to_l0(
    col_frame: &crate::dpb::RefFrame,
    ref_col: u8,
    l: usize,
    cur_l0_fns: &[u32],
) -> u8 {
    let fns = if l == 0 {
        &col_frame.ref_l0_fns
    } else {
        &col_frame.ref_l1_fns
    };
    let Some(&want) = fns.get(ref_col as usize) else {
        return 0;
    };
    for (j, &fn_) in cur_l0_fns.iter().enumerate() {
        if fn_ == want {
            return j as u8;
        }
    }
    0
}

/// Motion-compensates one partition for a B (or uni-pred P) MB: luma +
/// chroma samples into `buf`, choosing among
///   * single-list MC (`Some(list)` with `dirs`-bit set),
///   * bi-prediction average `(p0+p1+1)>>1`,
///   * explicit weighted bi-prediction (`wp_l0`/`wp_l1` tables),
///   * implicit weighted bi-prediction (`weighted_bipred_idc == 2`).
#[allow(clippy::too_many_arguments)]
fn mc_partition_bi(
    buf: &mut FrameBuf,
    cx: &SliceCx<'_>,
    dpb: &Dpb,
    x0: usize,
    y0: usize,
    pw: usize,
    ph: usize,
    mv0: [i32; 2],
    mv1: [i32; 2],
    ref0: i32,
    ref1: i32,
) -> Result<()> {
    debug_assert!(ref0 >= 0 || ref1 >= 0);
    let luma0 = if ref0 >= 0 {
        let rf = &dpb.refs[cx.ref_order[ref0 as usize]];
        let mut p = alloc::vec![0u8; pw * ph];
        inter::mc_luma(
            &rf.y,
            buf.w,
            buf.h,
            x0 as i32,
            y0 as i32,
            (mv0[0], mv0[1]),
            pw,
            ph,
            &mut p,
        );
        Some(p)
    } else {
        None
    };
    let luma1 = if ref1 >= 0 {
        let rf = &dpb.refs[cx.ref_order1[ref1 as usize]];
        let mut p = alloc::vec![0u8; pw * ph];
        inter::mc_luma(
            &rf.y,
            buf.w,
            buf.h,
            x0 as i32,
            y0 as i32,
            (mv1[0], mv1[1]),
            pw,
            ph,
            &mut p,
        );
        Some(p)
    } else {
        None
    };
    // Luma store/combine.
    let mut luma = alloc::vec![0u8; pw * ph];
    match (luma0, luma1) {
        (Some(mut p0), None) => {
            if let Some(wp) = &cx.h.wp_l0 {
                if let Some(e) = wp.get(ref0 as usize) {
                    if e.luma_flag {
                        inter::apply_wp(&mut p0, e.luma.0, e.luma.1, cx.h.wp_denom.0);
                    }
                }
            }
            luma.copy_from_slice(&p0);
        }
        (None, Some(mut p1)) => {
            if let Some(wp) = &cx.h.wp_l1 {
                if let Some(e) = wp.get(ref1 as usize) {
                    if e.luma_flag {
                        inter::apply_wp(&mut p1, e.luma.0, e.luma.1, cx.h.wp_denom.0);
                    }
                }
            }
            luma.copy_from_slice(&p1);
        }
        (Some(p0), Some(p1)) => {
            // Bi-prediction: explicit weights when both lists carry a
            // table, implicit (POC-derived) under weighted_bipred_idc
            // == 2, plain average otherwise.
            if cx.h.wp_l0.is_some() && cx.h.wp_l1.is_some() {
                let e0 = cx.h.wp_l0.as_ref().and_then(|w| w.get(ref0 as usize));
                let e1 = cx.h.wp_l1.as_ref().and_then(|w| w.get(ref1 as usize));
                let w0 = e0.map(|e| {
                    if e.luma_flag {
                        e.luma.0
                    } else {
                        1 << cx.h.wp_denom.0
                    }
                });
                let w1 = e1.map(|e| {
                    if e.luma_flag {
                        e.luma.0
                    } else {
                        1 << cx.h.wp_denom.0
                    }
                });
                let o = e0.map(|e| e.luma.1).unwrap_or(0) + e1.map(|e| e.luma.1).unwrap_or(0);
                let d = cx.h.wp_denom.0;
                for i in 0..pw * ph {
                    luma[i] = (((i32::from(p0[i]) * w0.unwrap_or(1 << d)
                        + i32::from(p1[i]) * w1.unwrap_or(1 << d))
                        + (((o + 1) | 1) << d))
                        >> (d + 1))
                        .clamp(0, 255) as u8;
                }
            } else if cx.pps.weighted_bipred_idc == 2 {
                // Implicit weights via the per-slice table on `cx`.
                for i in 0..pw * ph {
                    let w0 = cx.iw0(ref0 as usize, ref1 as usize);
                    luma[i] = (((i32::from(p0[i]) * w0 + i32::from(p1[i]) * (64 - w0)) + 32) >> 6)
                        .clamp(0, 255) as u8;
                }
            } else {
                for i in 0..pw * ph {
                    luma[i] = ((i32::from(p0[i]) + i32::from(p1[i]) + 1) >> 1) as u8;
                }
            }
        }
        (None, None) => unreachable!(),
    }
    for yy in 0..ph {
        buf.y[(y0 + yy) * buf.w + x0..(y0 + yy) * buf.w + x0 + pw]
            .copy_from_slice(&luma[yy * pw..yy * pw + pw]);
    }
    // Chroma.
    let (cx0, cy0, cw2, ch2) = (x0 / 2, y0 / 2, pw / 2, ph / 2);
    for c in 0..2usize {
        let mut t0 = if ref0 >= 0 {
            let rf = &dpb.refs[cx.ref_order[ref0 as usize]];
            let pl = if c == 0 { &rf.cb } else { &rf.cr };
            let mut p = alloc::vec![0u8; cw2 * ch2];
            inter::mc_chroma(
                pl,
                buf.w / 2,
                buf.h / 2,
                cx0 as i32,
                cy0 as i32,
                (mv0[0], mv0[1]),
                cw2,
                ch2,
                &mut p,
            );
            Some(p)
        } else {
            None
        };
        let mut t1 = if ref1 >= 0 {
            let rf = &dpb.refs[cx.ref_order1[ref1 as usize]];
            let pl = if c == 0 { &rf.cb } else { &rf.cr };
            let mut p = alloc::vec![0u8; cw2 * ch2];
            inter::mc_chroma(
                pl,
                buf.w / 2,
                buf.h / 2,
                cx0 as i32,
                cy0 as i32,
                (mv1[0], mv1[1]),
                cw2,
                ch2,
                &mut p,
            );
            Some(p)
        } else {
            None
        };
        let mut out = alloc::vec![0u8; cw2 * ch2];
        match (&mut t0, &mut t1) {
            (Some(p0), None) => {
                if let Some(wp) = &cx.h.wp_l0 {
                    if let Some(e) = wp.get(ref0 as usize) {
                        if e.chroma_flag {
                            inter::apply_wp(p0, e.chroma[c].0, e.chroma[c].1, cx.h.wp_denom.1);
                        }
                    }
                }
                out.copy_from_slice(p0);
            }
            (None, Some(p1)) => {
                if let Some(wp) = &cx.h.wp_l1 {
                    if let Some(e) = wp.get(ref1 as usize) {
                        if e.chroma_flag {
                            inter::apply_wp(p1, e.chroma[c].0, e.chroma[c].1, cx.h.wp_denom.1);
                        }
                    }
                }
                out.copy_from_slice(p1);
            }
            (Some(p0), Some(p1)) => {
                if cx.h.wp_l0.is_some() && cx.h.wp_l1.is_some() {
                    let e0 = cx.h.wp_l0.as_ref().and_then(|w| w.get(ref0 as usize));
                    let e1 = cx.h.wp_l1.as_ref().and_then(|w| w.get(ref1 as usize));
                    let w0 = e0
                        .map(|e| {
                            if e.chroma_flag {
                                e.chroma[c].0
                            } else {
                                1 << cx.h.wp_denom.1
                            }
                        })
                        .unwrap_or(1 << cx.h.wp_denom.1);
                    let w1 = e1
                        .map(|e| {
                            if e.chroma_flag {
                                e.chroma[c].0
                            } else {
                                1 << cx.h.wp_denom.1
                            }
                        })
                        .unwrap_or(1 << cx.h.wp_denom.1);
                    let o = e0.map(|e| e.chroma[c].1).unwrap_or(0)
                        + e1.map(|e| e.chroma[c].1).unwrap_or(0);
                    let d = cx.h.wp_denom.1;
                    for i in 0..cw2 * ch2 {
                        out[i] = (((i32::from(p0[i]) * w0 + i32::from(p1[i]) * w1)
                            + (((o + 1) | 1) << d))
                            >> (d + 1))
                            .clamp(0, 255) as u8;
                    }
                } else if cx.pps.weighted_bipred_idc == 2 {
                    for i in 0..cw2 * ch2 {
                        let w0 = cx.iw0(ref0 as usize, ref1 as usize);
                        out[i] = (((i32::from(p0[i]) * w0 + i32::from(p1[i]) * (64 - w0)) + 32)
                            >> 6)
                            .clamp(0, 255) as u8;
                    }
                } else {
                    for i in 0..cw2 * ch2 {
                        out[i] = ((i32::from(p0[i]) + i32::from(p1[i]) + 1) >> 1) as u8;
                    }
                }
            }
            (None, None) => unreachable!(),
        }
        let dst = if c == 0 { &mut buf.cb } else { &mut buf.cr };
        for yy in 0..ch2 {
            dst[(cy0 + yy) * (buf.w / 2) + cx0..(cy0 + yy) * (buf.w / 2) + cx0 + cw2]
                .copy_from_slice(&out[yy * cw2..yy * cw2 + cw2]);
        }
    }
    Ok(())
}

/// B direct-prediction motion derivation (spec 8.4.1.2, spatial and
/// temporal branches), ported from the reference decoder's
/// `pred_spatial_direct_motion` / `pred_temp_direct_motion` frame
/// paths. `grp_mask` selects which 8x8 groups are direct (a whole-MB
/// direct type passes `0b1111`; `B_8x8` passes only its direct
/// sub-types). For each selected group the function fills `m`'s
/// `mv`/`ref_idx`/`mv_l1`/`ref_idx_l1` arrays and runs
/// [`mc_partition_bi`].
#[allow(clippy::too_many_arguments)]
fn direct_motion(
    pic: &mut Pic,
    cx: &SliceCx<'_>,
    dpb: &Dpb,
    idx: usize,
    wm: usize,
    m: &mut MbState,
    grp_mask: u8,
) -> Result<()> {
    let map = MbMap {
        idx,
        x: idx % wm,
        y: idx / wm,
        width: wm,
        sid: cx.slice_id,
    };
    if cx.ref_order1.is_empty() {
        return Err(Error::BadValue("B direct with empty L1 list"));
    }
    let col_frame = &dpb.refs[cx.ref_order1[0]];
    let col_mb = col_frame.coloc.get(idx);
    let px0 = map.x * 16;
    let py0 = map.y * 16;
    let inference = cx.sps.direct_8x8_inference;

    if cx.h.direct_spatial {
        // ---- spatial direct (spec 8.4.1.2.2) ----
        // Per-list refIdx = min(non-negative neighbour refs) and MVP.
        let mut ref_l = [0i32; 2];
        let mut mv_l = [[0i16; 2]; 2];
        for l in 0..2usize {
            let a = mv_at_l(&pic.mbs, None, map, -1, 0, l);
            let b = mv_at_l(&pic.mbs, None, map, 0, -1, l);
            let mut c = mv_at_l(&pic.mbs, None, map, 4, -1, l);
            if c.is_none() {
                c = mv_at_l(&pic.mbs, None, map, -1, -1, l);
            }
            let min_r = [a, b, c]
                .iter()
                .filter_map(|n| n.map(|(_, r)| r))
                .filter(|&r| r >= 0)
                .min()
                .unwrap_or(-1);
            ref_l[l] = min_r;
            if min_r >= 0 {
                // Median-of-three MVP restricted: matches spec
                // "most common ref" selection.
                let (av, ar) = a.unwrap_or(([0, 0], -1));
                let (bv, br) = b.unwrap_or(([0, 0], -1));
                let (cv, cr) = c.unwrap_or(([0, 0], -1));
                let matches = [ar == min_r, br == min_r, cr == min_r];
                let cnt = matches.iter().filter(|&&f| f).count();
                // ffmpeg pred_spatial_direct_motion: match_count > 1
                // takes mid_pred over the RAW A/B/C vectors (no
                // zero-padding of non-matching neighbours); a single
                // match takes that neighbour verbatim.
                mv_l[l] = if cnt > 1 {
                    [med3(av[0], bv[0], cv[0]), med3(av[1], bv[1], cv[1])]
                } else if ar == min_r {
                    av
                } else if br == min_r {
                    bv
                } else {
                    cv
                };
            }
        }
        if ref_l[0] < 0 && ref_l[1] < 0 {
            // ffmpeg pred_spatial_direct_motion (h264_direct.c):
            // per-list -1 keeps that list unused in the MC; only when
            // both lists lack a non-negative neighbour refIdx do both
            // fall back to ref 0, mv 0 (spec 8.4.1.2.2).
            ref_l = [0, 0];
            mv_l = [[0, 0]; 2];
        }
        for g in 0..4usize {
            if grp_mask & (1 << g) == 0 {
                continue;
            }
            let gx = (g % 2) * 2;
            let gy = (g / 2) * 2;
            // Near-zero colocated check (spec 8.4.1.2.2 second
            // paragraph): intra colMB or near-zero colMv forces both
            // MVs to zero; reference decoder keeps non-zero-ref
            // predictions otherwise.
            let mut a = mv_l[0];
            let mut b = mv_l[1];
            if let Some(cm) = col_mb {
                let near = if !cm.intra {
                    let r0 = cm.ref_l0[g];
                    let r1 = cm.ref_l1[g];
                    if r0 == 0 {
                        let mv = cm.mv_l0[block_index(gx, gy)];
                        mv[0].abs() <= 1 && mv[1].abs() <= 1
                    } else {
                        r0 == 0xff && r1 == 0 && {
                            let mv = cm.mv_l1[block_index(gx, gy)];
                            mv[0].abs() <= 1 && mv[1].abs() <= 1
                        }
                    }
                } else {
                    false
                };
                if near || cm.intra {
                    a = if ref_l[0] > 0 { mv_l[0] } else { [0, 0] };
                    b = if ref_l[1] > 0 { mv_l[1] } else { [0, 0] };
                    if cm.intra {
                        a = mv_l[0];
                        b = mv_l[1];
                    }
                }
            }

            let (w4, h4) = if inference { (2, 2) } else { (1, 1) };
            for yy in 0..h4 * (if inference { 1 } else { 2 }) {
                for xx in 0..w4 * (if inference { 1 } else { 2 }) {
                    let (x4, y4) = (gx + xx, gy + yy);
                    let b4 = block_index(x4, y4);
                    m.mv[b4] = if ref_l[0] >= 0 { a } else { [0, 0] };
                    m.ref_idx[b4] = if ref_l[0] >= 0 { ref_l[0] as u8 } else { 0xff };
                    m.mv_valid[b4] = ref_l[0] >= 0;
                    m.mv_l1[b4] = if ref_l[1] >= 0 { b } else { [0, 0] };
                    m.ref_idx_l1[b4] = if ref_l[1] >= 0 { ref_l[1] as u8 } else { 0xff };
                }
            }
            if inference {
                mc_partition_bi(
                    &mut pic.buf,
                    cx,
                    dpb,
                    px0 + gx * 4,
                    py0 + gy * 4,
                    8,
                    8,
                    [i32::from(a[0]), i32::from(a[1])],
                    [i32::from(b[0]), i32::from(b[1])],
                    ref_l[0],
                    ref_l[1],
                )?;
            } else {
                for yy in 0..2usize {
                    for xx in 0..2usize {
                        let (x4, y4) = (gx + xx, gy + yy);
                        let mv_c = if let Some(cm) = col_mb {
                            // Per-4x4 near-zero check uses the colocated
                            // 4x4 mv (spec second-paragraph note).
                            let b4c = block_index(x4, y4);
                            if !cm.intra
                                && cm.ref_l0[g] == 0
                                && cm.mv_l0[b4c][0].abs() <= 1
                                && cm.mv_l0[b4c][1].abs() <= 1
                            {
                                let a2 = if ref_l[0] == 0 { [0, 0] } else { a };
                                let b2 = if ref_l[1] == 0 { [0, 0] } else { b };
                                (a2, b2)
                            } else {
                                (a, b)
                            }
                        } else {
                            (a, b)
                        };
                        mc_partition_bi(
                            &mut pic.buf,
                            cx,
                            dpb,
                            px0 + x4 * 4,
                            py0 + y4 * 4,
                            4,
                            4,
                            [i32::from(mv_c.0[0]), i32::from(mv_c.0[1])],
                            [i32::from(mv_c.1[0]), i32::from(mv_c.1[1])],
                            ref_l[0],
                            ref_l[1],
                        )?;
                    }
                }
            }
        }
        return Ok(());
    }

    // ---- temporal direct (spec 8.4.1.2.3) ----
    let col_frame = &dpb.refs[cx.ref_order1[0]];
    let col_mb = col_frame.coloc.get(idx);
    for g in 0..4usize {
        if grp_mask & (1 << g) == 0 {
            continue;
        }
        let gx = (g % 2) * 2;
        let gy = (g / 2) * 2;
        if inference {
            // One derivation per 8x8 group; colocated sample is the
            // group's bottom-right 4x4 (reference `l1mv[x8*3 + y8*3]`).
            let (r0, mv_c0, _mv_c1) = if let Some(cm) = col_mb {
                if cm.intra {
                    (0i32, [0i16; 2], [0i16; 2])
                } else {
                    let b4 = block_index(gx + 1, gy + 1);
                    let (list, rcol) = if cm.ref_l0[g] != 0xff {
                        (0usize, cm.ref_l0[g])
                    } else {
                        (1usize, cm.ref_l1[g])
                    };
                    let mv = if list == 0 {
                        cm.mv_l0[b4]
                    } else {
                        cm.mv_l1[b4]
                    };
                    let mv1 = if list == 0 {
                        cm.mv_l1[b4]
                    } else {
                        cm.mv_l0[b4]
                    };
                    let r0 = i32::from(col_ref_to_l0(col_frame, rcol, list, cx.ref_l0_fns));
                    (r0, mv, mv1)
                }
            } else {
                (0i32, [0i16; 2], [0i16; 2])
            };
            let scale = if let Some(cm) = col_mb {
                if cm.intra {
                    0
                } else {
                    let poc0 = dpb.refs[cx.ref_order[r0 as usize]].poc;
                    direct_dsf(cx.cur_poc, poc0, col_frame.poc)
                }
            } else {
                0
            };
            let mv0 = [
                ((scale * i32::from(mv_c0[0]) + 128) >> 8) as i16,
                ((scale * i32::from(mv_c0[1]) + 128) >> 8) as i16,
            ];
            let mv1 = [
                (i32::from(mv0[0]) - i32::from(mv_c0[0])) as i16,
                (i32::from(mv0[1]) - i32::from(mv_c0[1])) as i16,
            ];

            for yy in 0..2usize {
                for xx in 0..2usize {
                    let b4 = block_index(gx + xx, gy + yy);
                    m.mv[b4] = mv0;
                    m.ref_idx[b4] = r0 as u8;
                    m.mv_valid[b4] = true;
                    m.mv_l1[b4] = mv1;
                    m.ref_idx_l1[b4] = 0;
                }
            }
            mc_partition_bi(
                &mut pic.buf,
                cx,
                dpb,
                px0 + gx * 4,
                py0 + gy * 4,
                8,
                8,
                [i32::from(mv0[0]), i32::from(mv0[1])],
                [i32::from(mv1[0]), i32::from(mv1[1])],
                r0,
                0,
            )?;
        } else {
            // Per-4x4 derivation.
            for yy in 0..2usize {
                for xx in 0..2usize {
                    let (x4, y4) = (gx + xx, gy + yy);
                    let b4 = block_index(x4, y4);
                    let (r0, mv_c) = if let Some(cm) = col_mb {
                        if cm.intra {
                            (0i32, [0i16; 2])
                        } else {
                            let (list, rcol) = if cm.ref_l0[g] != 0xff {
                                (0usize, cm.ref_l0[g])
                            } else {
                                (1usize, cm.ref_l1[g])
                            };
                            let mv = if list == 0 {
                                cm.mv_l0[b4]
                            } else {
                                cm.mv_l1[b4]
                            };
                            let r0 = i32::from(col_ref_to_l0(col_frame, rcol, list, cx.ref_l0_fns));
                            (r0, mv)
                        }
                    } else {
                        (0i32, [0i16; 2])
                    };
                    let scale = if let Some(cm) = col_mb {
                        if cm.intra {
                            0
                        } else {
                            let poc0 = dpb.refs[cx.ref_order[r0 as usize]].poc;
                            direct_dsf(cx.cur_poc, poc0, col_frame.poc)
                        }
                    } else {
                        0
                    };
                    let mv0 = [
                        ((scale * i32::from(mv_c[0]) + 128) >> 8) as i16,
                        ((scale * i32::from(mv_c[1]) + 128) >> 8) as i16,
                    ];
                    let mv1 = [
                        (i32::from(mv0[0]) - i32::from(mv_c[0])) as i16,
                        (i32::from(mv0[1]) - i32::from(mv_c[1])) as i16,
                    ];
                    m.mv[b4] = mv0;
                    m.ref_idx[b4] = r0 as u8;
                    m.mv_valid[b4] = true;
                    m.mv_l1[b4] = mv1;
                    m.ref_idx_l1[b4] = 0;
                    mc_partition_bi(
                        &mut pic.buf,
                        cx,
                        dpb,
                        px0 + x4 * 4,
                        py0 + y4 * 4,
                        4,
                        4,
                        [i32::from(mv0[0]), i32::from(mv0[1])],
                        [i32::from(mv1[0]), i32::from(mv1[1])],
                        r0,
                        0,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// `mv_at` generalised to one list: list 1 reads `mv_l1`/`ref_idx_l1`
/// (spec 8.4.1.2.2 neighbour lookups are per prediction list).
fn mv_at_l(
    mbs: &[MbState],
    cur: Option<&MbState>,
    map: MbMap,
    x: i32,
    y: i32,
    l: usize,
) -> Option<([i16; 2], i32)> {
    if (0..4).contains(&x) && (0..4).contains(&y) {
        let nb = cur?;
        let b = block_index(x as usize, y as usize);
        if nb.mb_type.is_intra() {
            return Some(([0, 0], -1));
        }
        let (mvv, r) = if l == 0 {
            (nb.mv[b], nb.ref_idx[b])
        } else {
            (nb.mv_l1[b], nb.ref_idx_l1[b])
        };
        // Undecoded same-MB partitions are unavailable neighbours
        // (spec 8.4.1.1): returning a fake candidate would suppress the
        // C->D fallback below. A decoded-ref-but-MV-uncommitted block
        // (refs decode before mvds in P8x8) is likewise unavailable.
        // L1 has no validity flag yet — gate L0 only.
        if r == 0xff || l == 0 && !nb.mv_valid[b] {
            return None;
        }
        return Some((mvv, i32::from(r)));
    }
    let nidx = match (x < 0, y < 0) {
        (true, false) => map.mb_a(),
        (false, true) => {
            if x >= 4 {
                map.mb_c()
            } else {
                map.mb_b()
            }
        }
        (true, true) => map.mb_d(),
        (false, false) => None,
    }?;
    let nb = &mbs[nidx];
    if nb.slice_id != map.sid {
        return None;
    }
    let b = block_index(x.rem_euclid(4) as usize, y.rem_euclid(4) as usize);
    if nb.mb_type.is_intra() {
        return Some(([0, 0], -1));
    }
    let (mvv, r) = if l == 0 {
        (nb.mv[b], nb.ref_idx[b])
    } else {
        (nb.mv_l1[b], nb.ref_idx_l1[b])
    };
    if l == 0 && !nb.mv_valid[b] {
        return Some((mvv, -1));
    }
    Some((mvv, if r == 0xff { -1 } else { i32::from(r) }))
}

// ======================== CABAC macroblock layer ========================

/// CABAC counterpart of [`slice_data`]: skip flags instead of run
/// counts, per-MB decode, `end_of_slice_flag` terminate checks after
/// every MB (spec 9.3.2.1 / reference `ff_h264_decode_mb_cabac` loop).
fn slice_data_cabac(
    pic: &mut Pic,
    cx: &mut SliceCx<'_>,
    dpb: &Dpb,
    rbsp: &[u8],
    cabac_off: usize,
) -> Result<()> {
    let wm = cx.sps.width_mbs as usize;
    let total = (cx.sps.width_mbs * cx.sps.height_mbs) as usize;
    let mut mb_idx = cx.h.first_mb as usize;
    if mb_idx > total {
        return Err(Error::BadValue("first_mb_in_slice past picture"));
    }
    // Spec 9.3.1.1: context initialisation uses `SliceQPY` of THIS slice
    // (`pic_init_qp + slice_qp_delta`), not the carried `qp_prev` of the
    // previous slice's last MB — ffmpeg uses `sl->qscale` for the same.
    let slice_qp = (cx.pps.pic_init_qp + cx.h.slice_qp_delta).clamp(0, 51) as u8;
    let mut cab = cabac::Cabac::new(
        &rbsp[cabac_off..],
        cx.h.slice_type,
        slice_qp,
        cx.h.cabac_init_idc,
    );
    loop {
        // Skip flag (P/B): context counts non-skipped same-slice
        // neighbours; B slices shift the base by 13.
        let skipped = match cx.h.slice_type {
            SliceType::I => false,
            _ => {
                let base = if cx.h.slice_type == SliceType::B {
                    13
                } else {
                    0
                };
                let mut c = base;
                // EXPERIMENT: strict spec edge availability.
                if mb_idx % wm > 0 {
                    if let Some(a) = pic.mbs.get(mb_idx - 1) {
                        if a.slice_id == cx.slice_id && !a.skip {
                            c += 1;
                        }
                    }
                }
                if mb_idx >= wm {
                    if let Some(b) = pic.mbs.get(mb_idx - wm) {
                        if b.slice_id == cx.slice_id && !b.skip {
                            c += 1;
                        }
                    }
                }
                cab.mb_skip(c)?
            }
        };
        if skipped {
            match cx.h.slice_type {
                SliceType::P => p_skip(pic, cx, dpb, mb_idx, wm)?,
                SliceType::B => b_skip(pic, cx, dpb, mb_idx, wm)?,
                SliceType::I => unreachable!(),
            }
        } else {
            mb_decode_cabac(pic, &mut cab, cx, dpb, mb_idx, wm)?;
        }
        mb_idx += 1;
        if mb_idx >= total {
            break;
        }
        if cab.terminate()? {
            break;
        }
    }
    Ok(())
}

/// Full CABAC macroblock decode (syntax + reconstruction), mirroring
/// the CAVLC [`mb_decode`] but reading its symbols through the CABAC
/// engine and their neighbour contexts.
#[allow(clippy::too_many_lines)]
fn mb_decode_cabac(
    pic: &mut Pic,
    cab: &mut cabac::Cabac<'_>,
    cx: &mut SliceCx<'_>,
    dpb: &Dpb,
    idx: usize,
    wm: usize,
) -> Result<()> {
    let map = MbMap {
        idx,
        x: idx % wm,
        y: idx / wm,
        width: wm,
        sid: cx.slice_id,
    };

    // ---- mb_type (spec 9.3.2.5) ----
    let mb_type = match cx.h.slice_type {
        SliceType::I => {
            // ctx 3+min(2, intra16-neighbour-count).
            let mut cnt = 0usize;
            if nb_at(&pic.mbs, map.mb_a(), cx.slice_id)
                .is_some_and(|m| matches!(m.mb_type, MbType::I16x16 { .. } | MbType::IPcm))
            {
                cnt += 1;
            }
            if nb_at(&pic.mbs, map.mb_b(), cx.slice_id)
                .is_some_and(|m| matches!(m.mb_type, MbType::I16x16 { .. } | MbType::IPcm))
            {
                cnt += 1;
            }
            let code = cab.i_mb_type(cnt)?;
            MbType::i_slice(u32::from(code))?
        }
        SliceType::P => {
            // ctx 14 prefix: 0 = inter, 1 = intra tail at ctx 17.
            let code = cab.p_mb_type()?;
            if code >= 5 {
                MbType::i_slice(u32::from(code) - 5)?
            } else {
                MbType::p_slice(u32::from(code))?
            }
        }
        SliceType::B => {
            // ctx 27 + non-direct-neighbour count. "Direct" at MB level
            // means ONLY B_Direct_16x16 / B_Skip (ffmpeg IS_DIRECT =
            // MB_TYPE_DIRECT2); a B_8x8 whose SUB-partitions are direct
            // is NOT MB-level direct and must still count here.
            let mut cnt = 0usize;
            let nondirect =
                |m: &MbState| m.mb_type != MbType::BDirect && m.mb_type != MbType::BSkip;
            if nb_at(&pic.mbs, map.mb_a(), cx.slice_id).is_some_and(nondirect) {
                cnt += 1;
            }
            if nb_at(&pic.mbs, map.mb_b(), cx.slice_id).is_some_and(nondirect) {
                cnt += 1;
            }
            let code = cab.b_mb_type(cnt)?;

            if code >= 23 {
                MbType::i_slice(u32::from(code) - 23)?
            } else {
                MbType::b_slice(u32::from(code))?
            }
        }
    };

    let mut m = MbState::new();
    m.mb_type = mb_type;
    m.slice_id = cx.slice_id;
    m.dbg_idx = idx as u32;
    m.disable_deblock_idc = cx.h.disable_deblock_idc;
    m.filter_offset_a = cx.h.offset_a;
    m.filter_offset_b = cx.h.offset_b;
    m.direct_spatial = cx.h.direct_spatial;

    // ---- mb_pred / sub_mb_pred ----
    // `part`: (x4, y4, w4, h4) geometry; L0/L1 ref+mvd pairs.
    let mut inter_parts: Vec<Part> = Vec::new();
    let mut direct_mask = 0u8;
    // ffmpeg `get_dct8x8_allowed` (h264_mvpred.h:157): any sub-mb
    // partitioned below 8x8 (8x4/4x8/4x4) blocks
    // transform_size_8x8_flag; B_Direct also blocks it without
    // direct_8x8_inference.
    let mut sub_blocks_8x8 = false;
    match mb_type {
        MbType::I4x4 => {
            // ctxIdx 399 transform_size_8x8_flag (spec Table 9-11;
            // ctxIdxInc = neighbour 8x8 flags; spec 9.3.3.1.1.9).
            if cabac_t8x8_allowed(cx) && cab.transform_8x8(cabac_ctx399(&pic.mbs, &map))? {
                m.transform8x8 = true;
            }
            if m.transform8x8 {
                for blk in 0..4 {
                    let g = blk * 4;
                    let mpm = i4x4_mpm(&pic.mbs, &m, map, g);
                    // Same reader as 4x4 (ffmpeg decode_cabac_mb_
                    // intra4x4_pred_mode with i += 4): no skip-3.
                    let mode = cab.intra4x4_mode(mpm)?;
                    for i in 0..4 {
                        let (bx, by) = block_xy(g + i);
                        m.i4x4_modes[by * 4 + bx] = mode;
                    }
                }
            } else {
                for blk in 0..16 {
                    let (bx, by) = block_xy(blk);
                    let r = by * 4 + bx;
                    let mpm = i4x4_mpm(&pic.mbs, &m, map, blk);
                    let mode = cab.intra4x4_mode(mpm)?;
                    m.i4x4_modes[r] = mode;
                }
            }
            let cctx = cabac_chroma_ctx(&pic.mbs, &map);
            m.chroma_pred = cab.chroma_pred(cctx)?;
            if m.chroma_pred > 3 {
                return Err(Error::BadValue("intra_chroma_pred_mode over 3"));
            }
        }
        MbType::I16x16 { .. } => {
            let cctx = cabac_chroma_ctx(&pic.mbs, &map);
            m.chroma_pred = cab.chroma_pred(cctx)?;
            if m.chroma_pred > 3 {
                return Err(Error::BadValue("intra_chroma_pred_mode over 3"));
            }
        }
        MbType::IPcm => {
            // I_PCM carries no intra_chroma_pred_mode (spec 7.3.5
            // mb_pred only emits it for the NxN/16x16 intra types).
        }
        MbType::BDirect => {
            // Whole-MB direct: motion derived, no residual motion bits.
            direct_mask = 0b1111;
        }
        MbType::B16x16 { dirs } => {
            let mut p = Part::new(0, 0, 4, 4, dirs);
            cabac_read_refs_mvds(pic, cab, cx, map, &mut m, core::slice::from_mut(&mut p), 1)?;
            inter_parts.push(p);
        }
        MbType::BPart { p0, p1 } => {
            let mut ps = [Part::new(0, 0, p0.0 as usize, p0.1 as usize, p0.2), {
                let g = part_geo_b(0, p0.0, p0.1);
                Part::new(g.0, g.1, g.2, g.3, p1.2)
            }];
            cabac_read_refs_mvds(pic, cab, cx, map, &mut m, &mut ps, 2)?;
            inter_parts.extend_from_slice(&ps);
        }
        MbType::B8x8 => {
            let mut subs = [SubMbType::BDirect; 4];
            for st in subs.iter_mut() {
                *st = SubMbType::from_code_b(u32::from(cab.b_sub_type()?))?;
            }
            // x264 writes the flag only when every sub is an 8x8
            // partition (BInter w4==h4==2); direct subs block without
            // direct_8x8_inference.
            for st in &subs {
                match st {
                    SubMbType::BDirect => {
                        if !cx.sps.direct_8x8_inference {
                            sub_blocks_8x8 = true;
                        }
                    }
                    SubMbType::BInter { w4, h4, .. } if !(w4 == &2 && h4 == &2) => {
                        sub_blocks_8x8 = true;
                    }
                    _ => {}
                }
            }
            // ffmpeg runs pred_direct_motion immediately after the
            // sub-mb-type reads (h264_cabac.c:2124), BEFORE ref_idx and
            // mvd: the direct groups' mv/ref caches are populated so a
            // later sub-partition's MVP (and the ref/amvd contexts)
            // already see the direct motion — e.g. sub2's MVP in a
            // B_8x8 reads the 4x4 left of (0,2) = direct sub1's derived
            // mv, which is unavailable if direct motion is deferred to
            // reconstruction.
            for (sm, st) in subs.iter().enumerate() {
                if *st == SubMbType::BDirect {
                    direct_mask |= 1 << sm;
                }
            }
            if direct_mask != 0 {
                direct_motion(pic, cx, dpb, idx, wm, &mut m, direct_mask)?;
            }
            // ref_idx per direction-bearing sub-mb (spec Table 7-18:
            // direct subs carry no ref/mvd).
            let mut refs0 = [0u8; 4];
            let mut refs1 = [0u8; 4];
            read_cabac_refs(pic, cab, cx, map, &mut m, &subs, &mut refs0, &mut refs1)?;
            for (sm, &st) in subs.iter().enumerate() {
                let sx = (sm % 2) * 2;
                let sy = (sm / 2) * 2;
                if st == SubMbType::BDirect {
                    direct_mask |= 1 << sm;
                    continue;
                }
                if let SubMbType::BInter {
                    w4,
                    h4,
                    nparts,
                    dirs,
                } = st
                {
                    for pi in 0..nparts as usize {
                        let (px, py) = b_part_xy(st, pi);
                        let mut p = Part::new(sx + px, sy + py, w4 as usize, h4 as usize, dirs);
                        p.ref0 = refs0[sm];
                        p.ref1 = refs1[sm];
                        inter_parts.push(p);
                    }
                }
            }
            // mvd reads: all L0 across every sub-partition, then all
            // L1 (spec 7.3.5.1's per-list order; the reference
            // decoder's `for(list){for(i){for(j)}}` loop).
            let base = inter_parts.len();
            for (sm, &st) in subs.iter().enumerate() {
                if let SubMbType::BInter { nparts, .. } = st {
                    let _ = nparts;
                    let _ = sm;
                }
            }
            for l in 0..2usize {
                for p in inter_parts.iter_mut().skip(base - b_part_count(&subs)) {
                    if p.dirs & (1 << l) != 0 {
                        cabac_read_mvd_one(pic, cab, cx, map, &mut m, p, l)?;
                    }
                }
            }
        }
        MbType::P8x8 | MbType::P8x8Ref0 => {
            let mut sub_types = [SubMbType::S8x8; 4];
            for st in sub_types.iter_mut() {
                *st = SubMbType::from_code(u32::from(cab.p_sub_type()?))?;
            }
            // ffmpeg ff_h264_p_sub_mb_type_info (h264data.c): sub code
            // 3 (4x4 partitions) maps to MB_TYPE_8x8 which *blocks*
            // the flag — the flag bit exists only when every sub is
            // an 8x8 partition.
            if sub_types.iter().any(|st| !matches!(st, SubMbType::S8x8)) {
                sub_blocks_8x8 = true;
            }
            let mut refs = [0u8; 4];
            if cx.h.num_ref_idx_l0_active > 1 && mb_type != MbType::P8x8Ref0 {
                for (sm, r) in refs.iter_mut().enumerate() {
                    let (x4, y4) = ((sm % 2) * 2, (sm / 2) * 2);
                    let ctx_i = cabac_ref_ctx(&pic.mbs, &m, map, x4, y4, 0);
                    *r = cab.ref_idx(ctx_i)? as u8;
                    if *r as usize >= cx.h.num_ref_idx_l0_active as usize {
                        return Err(Error::BadValue("ref_idx_l0 out of list"));
                    }
                    // Commit before the next sub-mb read: ref_idx ctx
                    // for a later partition sees earlier same-MB
                    // values (reference `fill_rectangle(ref_cache)`).
                    for yy in y4..y4 + 2 {
                        for xx in x4..x4 + 2 {
                            m.ref_idx[block_index(xx, yy)] = *r;
                        }
                    }
                }
            }
            for (sm, &st) in sub_types.iter().enumerate() {
                let (sizes, nparts) = st.parts();
                let _ = sizes;
                let sx = (sm % 2) * 2;
                let sy = (sm / 2) * 2;
                for pi in 0..nparts {
                    let (x4, y4, w4, h4) = sub_part_geo(sx, sy, st, pi);
                    let mut p = Part::new(x4, y4, w4, h4, 1);
                    p.ref0 = refs[sm];
                    cabac_read_mvd_one(pic, cab, cx, map, &mut m, &mut p, 0)?;
                    inter_parts.push(p);
                }
            }
        }
        MbType::P16x16 | MbType::P16x8 | MbType::P8x16 => {
            let (nparts, parts): (usize, [(usize, usize, usize, usize); 2]) = match mb_type {
                MbType::P16x16 => (1, [(0, 0, 4, 4), (0, 0, 0, 0)]),
                MbType::P16x8 => (2, [(0, 0, 4, 2), (0, 2, 4, 2)]),
                _ => (2, [(0, 0, 2, 4), (2, 0, 2, 4)]),
            };
            let mut refs = [0u8; 2];
            if cx.h.num_ref_idx_l0_active > 1 {
                for (pi, r) in refs.iter_mut().take(nparts).enumerate() {
                    let (x4, y4, w4, h4) = parts[pi];
                    let ctx_i = cabac_ref_ctx(&pic.mbs, &m, map, x4, y4, 0);
                    *r = cab.ref_idx(ctx_i)? as u8;
                    let _ = (x4, y4, w4, h4);
                    if *r as usize >= cx.h.num_ref_idx_l0_active as usize {
                        return Err(Error::BadValue("ref_idx_l0 out of list"));
                    }
                    // Commit before the next partition read (same-MB
                    // neighbour ctx reads it — reference `ref_cache`).
                    for yy in y4..y4 + h4 {
                        for xx in x4..x4 + w4 {
                            m.ref_idx[block_index(xx, yy)] = *r;
                        }
                    }
                }
            }
            for (pi, &(x4, y4, w4, h4)) in parts.iter().take(nparts).enumerate() {
                let mut p = Part::new(x4, y4, w4, h4, 1);
                p.ref0 = refs[pi];
                cabac_read_mvd_one(pic, cab, cx, map, &mut m, &mut p, 0)?;
                inter_parts.push(p);
            }
        }
        MbType::PSkip | MbType::BSkip => unreachable!("skip handled by the loop"),
    }
    m.direct_mask = direct_mask;

    // ---- coded_block_pattern + transform_8x8 + qp_delta ----
    // B_Direct_16x16 DOES code cbp (spec 7.3.5.1: coded_block_pattern
    // is emitted whenever MbPartPredMode != Intra_16x16/I_PCM — the
    // Direct exemption covers only mb_pred's ref_idx/mvd fields;
    // ffmpeg h264_cabac.c gate is `!IS_INTRA16x16(mb_type)` alone).
    let (mut cbp_luma, mut cbp_chroma) = (0u8, 0u8);
    if mb_type != MbType::IPcm {
        if let MbType::I16x16 {
            cbp_chroma: cc,
            cbp_luma: cl,
            ..
        } = mb_type
        {
            cbp_luma = cl;
            cbp_chroma = cc;
        } else {
            // Missing neighbour defaults: the reference decoder uses
            // 0x00F (inter) / 0x7CF (intra) — both make every context
            // contribution 0. Passing 0 instead flips ctx to +1/+2 and
            // desyncs the engine.
            let cbp_a = nb_at(&pic.mbs, map.mb_a(), cx.slice_id).map_or(0x0f, |n| n.cbp);
            let cbp_b = nb_at(&pic.mbs, map.mb_b(), cx.slice_id).map_or(0x0f, |n| n.cbp);
            // The cbp context bins address the neighbour column/row
            // adjacent to this MB. ffmpeg `left_cbp` (h264_mvpred.h
            // fill_decode_caches): `(cbp_table & 0x7F0)` keeps chroma +
            // I16x16-DC bits, and the two column folds are the identity
            // on the progressive left_block table (left_block[0]=0,
            // left_block[2]=2 map raw bits 1/3 back to context bits
            // 1/3). `MbState::cbp` is u8 (DC bits live in `dc_coded`),
            // so the in-range part is `cbp & 0x7A` (bits 1,3 luma;
            // 4,5 chroma; 6 spare). The previous bit-rotating transform
            // (0x05|shifts) cleared bit 3 and desynced the 4th luma-cbp
            // bin whenever the left MB was coded. The 0x0F unavailable
            // default stays as-is — the transform must only run on a
            // real neighbour's pattern.
            let cbp_a_ctx = if nb_at(&pic.mbs, map.mb_a(), cx.slice_id).is_some() {
                cbp_a & 0x7A
            } else {
                0x0f
            };
            let (ca, cb2) = (cbp_a_ctx, cbp_b);
            cbp_luma = cab.cbp_luma(ca, cb2)?;
            cbp_chroma = cab.cbp_chroma(cbp_a, cbp_b)?;
        }
    }
    m.cbp = (cbp_chroma << 4) | cbp_luma;
    // transform_size_8x8_flag (ffmpeg h264_cabac.c:2347:
    // `dct8x8_allowed && (cbp&15) && !IS_INTRA`; dct8x8_allowed is the
    // PPS flag narrowed by get_dct8x8_allowed for P_8x8/B_8x8).
    let mut t8x8 = mb_type.is_intra() && m.transform8x8;
    if !mb_type.is_intra()
        && cx.pps.transform_8x8_mode
        && cbp_luma > 0
        && !sub_blocks_8x8
        && (!matches!(mb_type, MbType::BDirect) || cx.sps.direct_8x8_inference)
    {
        let ctx399 =
            usize::from(nb_at(&pic.mbs, map.mb_a(), cx.slice_id).is_some_and(|n| n.transform8x8))
                + usize::from(
                    nb_at(&pic.mbs, map.mb_b(), cx.slice_id).is_some_and(|n| n.transform8x8),
                );
        t8x8 = cab.transform_8x8(ctx399)?;
    }
    m.transform8x8 = t8x8;
    let coded = cbp_luma > 0 || cbp_chroma > 0 || matches!(mb_type, MbType::I16x16 { .. });
    let mut qp_y = cx.qp_prev;
    if coded && mb_type != MbType::IPcm {
        let delta = cab.qp_delta(cx.qp_delta_prev)?;
        qp_y = (i64::from(cx.qp_prev) + i64::from(delta)).rem_euclid(52) as i32;
        cx.qp_prev = qp_y;
        cx.qp_delta_prev = delta;
    } else {
        cx.qp_delta_prev = 0;
    }
    if mb_type == MbType::IPcm {
        qp_y = 0;
        cx.qp_prev = 0;
        cx.qp_delta_prev = 0;
    }
    m.qp_y = qp_y.clamp(0, 51) as u8;

    // ---- residual parse ----
    let mut nz_acc = [0u8; 24];
    let mut luma_res = [[0i32; 16]; 16];
    let mut luma_res8 = [[0i32; 64]; 4];
    let mut dc_y = [0i32; 16];
    let mut chroma_ac = [[[0i32; 16]; 4]; 2];
    let mut chroma_dc = [[0i32; 4]; 2];
    if mb_type != MbType::IPcm {
        if let MbType::I16x16 { .. } = mb_type {
            // Luma DC (cat 0): cbf context from neighbours' DC-coded
            // bit, then 16 coefficients on the 4x4 scan. I16x16 is
            // always intra: missing neighbours default DC-coded (0x7CF).
            let cbf_ctx = dc_cbf_ctx(&pic.mbs, map, 0, true);
            let r = cabac::decode_residual(cab, ResCat::LumaDc16x16, cbf_ctx, &ZIGZAG_4X4)?;
            for s in 0..16 {
                dc_y[ZIGZAG_4X4[s] as usize] = r.levels[s];
            }
            if r.total_coeff > 0 {
                m.dc_coded |= 1;
            }
            for g in 0..4usize {
                if cbp_luma & (1 << g) == 0 {
                    continue;
                }
                for s4 in 0..4usize {
                    let blk = g * 4 + s4;
                    let cbf_ctx = nc_cbf_ctx(
                        &pic.mbs,
                        &nz_acc,
                        map,
                        blk,
                        Plane::Luma,
                        m.mb_type.is_intra(),
                    );
                    let r = cabac::decode_residual(
                        cab,
                        ResCat::LumaAc16x16,
                        cbf_ctx,
                        &ZIGZAG_4X4[1..],
                    )?;
                    for s in 0..15 {
                        luma_res[blk][ZIGZAG_4X4[s + 1] as usize] = r.levels[s];
                    }
                    nz_acc[blk] = r.total_coeff;
                }
            }
        } else if t8x8 {
            for (g, res8) in luma_res8.iter_mut().enumerate() {
                if cbp_luma & (1 << g) == 0 {
                    continue;
                }
                let blk = g * 4;
                // No cbf bin for luma 8x8 (ffmpeg ff_h264_cabac.c:1859);
                // the cbp luma bit already gated this group.
                let r = cabac::decode_residual(cab, ResCat::Luma8x8, 0, &ZIGZAG_8X8)?;
                // ZIGZAG_8X8 yields raster positions (row*8+col), matching
                // ff's zigzag_scan8x8; dequant_8x8/inverse_8x8/consumer
                // are all raster — place directly, no transpose.
                for (s, &z8) in ZIGZAG_8X8.iter().enumerate() {
                    res8[z8 as usize] = r.levels[s];
                }
                for s4 in 0..4usize {
                    nz_acc[blk + s4] = r.total_coeff.min(63);
                }
            }
        } else if cbp_luma > 0 {
            for g in 0..4usize {
                if cbp_luma & (1 << g) == 0 {
                    continue;
                }
                for s4 in 0..4usize {
                    let blk = g * 4 + s4;
                    let cbf_ctx = nc_cbf_ctx(
                        &pic.mbs,
                        &nz_acc,
                        map,
                        blk,
                        Plane::Luma,
                        m.mb_type.is_intra(),
                    );
                    let r = cabac::decode_residual(cab, ResCat::Luma4x4, cbf_ctx, &ZIGZAG_4X4)?;
                    for s in 0..16 {
                        luma_res[blk][ZIGZAG_4X4[s] as usize] = r.levels[s];
                    }
                    nz_acc[blk] = r.total_coeff;
                }
            }
        }
        if cbp_chroma > 0 {
            for (c, dc) in chroma_dc.iter_mut().enumerate() {
                let cbf_ctx = dc_cbf_ctx(&pic.mbs, map, 1 + c, mb_type.is_intra());
                let r = cabac::decode_residual(cab, ResCat::ChromaDc, cbf_ctx, &SCAN_2X2)?;
                *dc = [r.levels[0], r.levels[1], r.levels[2], r.levels[3]];
                if r.total_coeff > 0 {
                    m.dc_coded |= 1 << (1 + c);
                }
            }
            if cbp_chroma == 2 {
                for c in 0..2usize {
                    for b in 0..4usize {
                        let blk = b;
                        let cbf_ctx = nc_cbf_ctx(
                            &pic.mbs,
                            &nz_acc,
                            map,
                            blk,
                            Plane::chroma(c),
                            m.mb_type.is_intra(),
                        );
                        let r = cabac::decode_residual(
                            cab,
                            ResCat::ChromaAc,
                            cbf_ctx,
                            &ZIGZAG_4X4[1..],
                        )?;
                        for s in 0..15 {
                            chroma_ac[c][b][ZIGZAG_4X4[s + 1] as usize] = r.levels[s];
                        }
                        nz_acc[16 + c * 4 + b] = r.total_coeff;
                    }
                }
            }
        }
    }
    m.nz = nz_acc;

    // ---- reconstruction ----
    let px0 = map.x * 16;
    let py0 = map.y * 16;
    match mb_type {
        MbType::IPcm => {
            cab.byte_align();
            for row in 0..16 {
                for x in 0..16 {
                    pic.buf.y[(py0 + row) * pic.buf.w + px0 + x] = cab.byte()?;
                }
            }
            for row in 0..8 {
                for x in 0..8 {
                    let o = (py0 / 2 + row) * (pic.buf.w / 2) + px0 / 2 + x;
                    pic.buf.cb[o] = cab.byte()?;
                    pic.buf.cr[o] = cab.byte()?;
                }
            }
            cab.restart();
            m.nz = [16; 24];
            m.dc_coded = 0b111;
            // Neighbour-visible cbp: all luma groups + chroma coded
            // (reference `cbp_table = 0xf7ef`, low bits 0x2F).
            m.cbp = 0x2f;
        }
        MbType::I4x4 => {
            if t8x8 {
                for (g, raw_resid) in luma_res8.iter().enumerate() {
                    let nb = gather_luma8x8(&pic.buf, &pic.mbs, map, g, cx.pps);
                    // Top-left 4x4 cell of this 8x8 group on the raster
                    // mode grid: y*4 + x with (x, y) = (2*(g%2), 2*(g/2)).
                    let mode = m.i4x4_modes[8 * (g / 2) + 2 * (g % 2)];
                    let mut pred = [0u8; 64];
                    intra::pred8x8(mode, &nb, &mut pred);

                    let mut resid = *raw_resid;
                    if coded && cbp_luma & (1 << g) != 0 {
                        transform::dequant_8x8(&mut resid, m.qp_y);
                        transform::inverse_8x8(&mut resid);
                    }
                    let (bx, by) = ((g % 2) * 8, (g / 2) * 8);
                    for yy in 0..8 {
                        for xx in 0..8 {
                            let v = i32::from(pred[yy * 8 + xx]) + resid[yy * 8 + xx];
                            pic.buf.y[(py0 + by + yy) * pic.buf.w + px0 + bx + xx] =
                                v.clamp(0, 255) as u8;
                        }
                    }
                }
            } else {
                for (blk, raw_resid) in luma_res.iter().enumerate() {
                    let (bx, by) = block_xy(blk);
                    let r = by * 4 + bx;
                    let nb = gather4x4(&pic.buf, &pic.mbs, map, bx, by, blk, cx.pps);
                    let mut pred = [0u8; 16];
                    intra::pred4x4(m.i4x4_modes[r], &nb, &mut pred)?;
                    let mut resid = *raw_resid;
                    if coded {
                        transform::dequant_4x4(&mut resid, m.qp_y);
                        transform::inverse_4x4(&mut resid)?;
                    }
                    for yy in 0..4 {
                        for xx in 0..4 {
                            let v = i32::from(pred[yy * 4 + xx]) + resid[yy * 4 + xx];
                            pic.buf.y[(py0 + by * 4 + yy) * pic.buf.w + px0 + bx * 4 + xx] =
                                v.clamp(0, 255) as u8;
                        }
                    }
                }
            }
            chroma_recon(
                pic, cx, map, &m, &chroma_ac, &chroma_dc, cbp_chroma, px0, py0,
            )?;
        }
        MbType::I16x16 { pred, .. } => {
            let nb = gather16x16(&pic.buf, &pic.mbs, map, cx.pps);
            let mut out = [0u8; 256];
            intra::pred16x16(pred, &nb, &mut out)?;
            let mut dc = dc_y;
            transform::inv_luma_dc(&mut dc, m.qp_y);
            for (blk, raw_resid) in luma_res.iter().enumerate() {
                let (bx, by) = block_xy(blk);
                let mut resid = *raw_resid;
                resid[0] = dc[by * 4 + bx];
                if cbp_luma & (1 << (blk / 4)) != 0 {
                    for (i, v) in resid.iter_mut().enumerate().skip(1) {
                        let (x, y) = (i % 4, i / 4);
                        *v = transform::dequant_coeff(*v, m.qp_y, x, y);
                    }
                }
                transform::inverse_4x4(&mut resid)?;
                for yy in 0..4 {
                    for xx in 0..4 {
                        let v =
                            i32::from(out[(by * 4 + yy) * 16 + bx * 4 + xx]) + resid[yy * 4 + xx];
                        pic.buf.y[(py0 + by * 4 + yy) * pic.buf.w + px0 + bx * 4 + xx] =
                            v.clamp(0, 255) as u8;
                    }
                }
            }
            chroma_recon(
                pic, cx, map, &m, &chroma_ac, &chroma_dc, cbp_chroma, px0, py0,
            )?;
        }
        _ => {
            // Direct groups first: they own their MV/ref state and MC.
            if direct_mask != 0 {
                let mut dm = m.clone();
                dm.direct_mask = direct_mask;
                let r = direct_motion(pic, cx, dpb, idx, wm, &mut dm, direct_mask);
                m = dm;
                r?;
            }
            for p in &inter_parts {
                reconstruct_part(pic, cx, dpb, map, &mut m, *p)?;
            }
            // Luma residual.

            if m.transform8x8 {
                for (g, raw_resid) in luma_res8.iter().enumerate() {
                    if raw_resid.iter().all(|&v| v == 0) {
                        continue;
                    }
                    let (bx, by) = ((g % 2) * 8, (g / 2) * 8);
                    let mut resid = *raw_resid;
                    transform::dequant_8x8(&mut resid, m.qp_y);

                    transform::inverse_8x8(&mut resid);
                    let o = (py0 + by) * pic.buf.w + px0 + bx;
                    transform::add_residual_8x8(&mut pic.buf.y[o..], pic.buf.w, &resid);
                }
            } else {
                for (blk, raw_resid) in luma_res.iter().enumerate() {
                    if raw_resid.iter().all(|&v| v == 0) {
                        continue;
                    }

                    let (bx, by) = block_xy(blk);
                    let mut resid = *raw_resid;
                    transform::dequant_4x4(&mut resid, m.qp_y);

                    transform::inverse_4x4(&mut resid)?;

                    let o = (py0 + by * 4) * pic.buf.w + px0 + bx * 4;
                    transform::add_residual_4x4(&mut pic.buf.y[o..], pic.buf.w, &resid);
                }
            }
            chroma_recon(
                pic, cx, map, &m, &chroma_ac, &chroma_dc, cbp_chroma, px0, py0,
            )?;
        }
    }

    pic.mbs[idx] = m;
    Ok(())
}

/// One coded motion partition (B or P): geometry on the 4x4 grid plus
/// per-list `(ref_idx, mvd)` state. `dirs` is bit0 = L0, bit1 = L1.
#[derive(Copy, Clone)]
struct Part {
    x4: usize,
    y4: usize,
    w4: usize,
    h4: usize,
    dirs: u8,
    ref0: u8,
    ref1: u8,
    mvd0: [i16; 2],
    mvd1: [i16; 2],
}

impl Part {
    fn new(x4: usize, y4: usize, w4: usize, h4: usize, dirs: u8) -> Part {
        Part {
            x4,
            y4,
            w4,
            h4,
            dirs,
            ref0: 0,
            ref1: 0,
            mvd0: [0, 0],
            mvd1: [0, 0],
        }
    }
}

/// Geometries of a B two-part MB (B_L0/L1/Bi_16x8 or _8x16): partition
/// 0 always starts at the MB origin; partition 1 sits below a 16x8
/// split or right of an 8x16 split. `w4`,`h4` is partition-0's size.
fn part_geo_b(_first: u8, w4: u8, h4: u8) -> (usize, usize, usize, usize) {
    if h4 < w4 {
        (0, h4 as usize, w4 as usize, h4 as usize)
    } else {
        (w4 as usize, 0, w4 as usize, h4 as usize)
    }
}

/// Number of coded sub-partitions a B_8x8 MB's four sub-mbs produce
/// (direct sub-mbs produce none).
fn b_part_count(subs: &[SubMbType; 4]) -> usize {
    subs.iter()
        .map(|st| match st {
            SubMbType::BInter { nparts, .. } => *nparts as usize,
            _ => 0,
        })
        .sum()
}

/// (x4, y4) of sub-partition `pi` inside an 8x8 B sub-mb of type `st`
/// (relative to the sub-mb's top-left 4x4 corner).
fn b_part_xy(st: SubMbType, pi: usize) -> (usize, usize) {
    if let SubMbType::BInter { w4, h4, .. } = st {
        // Same tiling the P table uses: w×h partitions row-major.
        match (w4, h4) {
            (2, 2) => (0, 0),
            (2, 1) => (0, pi),
            (1, 2) => (pi, 0),
            _ => (pi % 2, pi / 2),
        }
    } else {
        (0, 0)
    }
}

/// `ref_idx` ctxIdx for the CABAC `ref_idx_lX` syntax element
/// (spec 9.3.3.1.1.6): counts neighbours with `ref_idx > 0` on the
/// partition's left/top edges, penalising direct/skipped blocks.
#[allow(clippy::too_many_arguments)]
fn cabac_ref_ctx(
    mbs: &[MbState],
    m: &MbState,
    map: MbMap,
    x4: usize,
    y4: usize,
    l: usize,
) -> usize {
    let mut ctx = 0usize;
    // Left neighbour of the partition's left edge.
    let (a_ref, a_direct) = if x4 > 0 {
        let b4 = block_index(x4 - 1, y4);
        let r = if l == 0 {
            m.ref_idx[b4]
        } else {
            m.ref_idx_l1[b4]
        };
        let g = (x4 - 1) / 2 + (y4 / 2) * 2;
        (r, m.direct_mask & (1 << g) != 0)
    } else if let Some(n) = map.mb_a().and_then(|i| mbs.get(i)) {
        if n.slice_id == map.sid {
            let b4 = block_index(3, y4);
            let r = if l == 0 {
                n.ref_idx[b4]
            } else {
                n.ref_idx_l1[b4]
            };
            let g = 1 + (y4 / 2) * 2;
            (
                r,
                n.direct_mask & (1 << g) != 0 || n.mb_type == MbType::BSkip,
            )
        } else {
            (0xff, false)
        }
    } else {
        (0xff, false)
    };
    // Top neighbour of the partition's top edge.
    let (b_ref, b_direct) = if y4 > 0 {
        let b4 = block_index(x4, y4 - 1);
        let r = if l == 0 {
            m.ref_idx[b4]
        } else {
            m.ref_idx_l1[b4]
        };
        let g = x4 / 2 + ((y4 - 1) / 2) * 2;
        (r, m.direct_mask & (1 << g) != 0)
    } else if let Some(n) = map.mb_b().and_then(|i| mbs.get(i)) {
        if n.slice_id == map.sid {
            let b4 = block_index(x4, 3);
            let r = if l == 0 {
                n.ref_idx[b4]
            } else {
                n.ref_idx_l1[b4]
            };
            let g = x4 / 2 + 2;
            (
                r,
                n.direct_mask & (1 << g) != 0 || n.mb_type == MbType::BSkip,
            )
        } else {
            (0xff, false)
        }
    } else {
        (0xff, false)
    };
    if a_ref != 0xff && a_ref > 0 && !a_direct {
        ctx += 1;
    }
    if b_ref != 0xff && b_ref > 0 && !b_direct {
        ctx += 2;
    }
    ctx
}

/// CABAC `ref_idx` reads for the sub-mb list (B_8x8): L0 for every
/// non-direct sub-mb first, then L1 — matching the spec's per-list
/// order inside `sub_mb_pred`.
#[allow(clippy::too_many_arguments)]
fn read_cabac_refs(
    pic: &mut Pic,
    cab: &mut cabac::Cabac<'_>,
    cx: &SliceCx<'_>,
    map: MbMap,
    m: &mut MbState,
    subs: &[SubMbType; 4],
    refs0: &mut [u8; 4],
    refs1: &mut [u8; 4],
) -> Result<()> {
    for l in 0..2usize {
        let need = if l == 0 {
            cx.h.num_ref_idx_l0_active > 1
        } else {
            cx.h.num_ref_idx_l1_active > 1
        };
        if !need {
            continue;
        }
        for (sm, st) in subs.iter().enumerate() {
            let (sx, sy) = ((sm % 2) * 2, (sm / 2) * 2);
            let uses = match st {
                SubMbType::BDirect => false,
                SubMbType::BInter { dirs, .. } => dirs & (1 << l) != 0,
                _ => l == 0, // P sub-mbs (unreachable on B)
            };
            if !uses {
                continue;
            }
            let ctx_i = cabac_ref_ctx(&pic.mbs, m, map, sx, sy, l);
            let _count = if l == 0 {
                cx.h.num_ref_idx_l0_active
            } else {
                cx.h.num_ref_idx_l1_active
            };
            let r = cab.ref_idx(ctx_i)? as u8;
            let list_len = if l == 0 {
                cx.ref_order.len()
            } else {
                cx.ref_order1.len()
            };
            if r as usize >= list_len {
                return Err(Error::BadValue("ref_idx out of list"));
            }
            if l == 0 {
                refs0[sm] = r;
            } else {
                refs1[sm] = r;
            }
            // Commit immediately: the NEXT sub-mb's ctx reads this
            // value (the reference decoder's ref_cache updates in
            // decode order).
            for yy in sy..sy + 2 {
                for xx in sx..sx + 2 {
                    let b = block_index(xx, yy);
                    if l == 0 {
                        m.ref_idx[b] = r;
                    } else {
                        m.ref_idx_l1[b] = r;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Reads one list's MVD for a partition through CABAC (ctxIdxInc from
/// neighbour `abs(mvd)` sums, spec 9.3.3.1.1.7) and stores the
/// magnitudes for future neighbours.
#[allow(clippy::too_many_arguments)]
fn cabac_read_mvd_one(
    pic: &mut Pic,
    cab: &mut cabac::Cabac<'_>,
    cx: &SliceCx<'_>,
    map: MbMap,
    m: &mut MbState,
    p: &mut Part,
    l: usize,
) -> Result<()> {
    let _ = cx;
    debug_assert!(p.dirs & (1 << l) != 0);
    // The `amvd` context sums |mvd| of the *predictors* cached for the
    // neighbouring blocks, NOT the decoded deltas: the reference
    // decoder fills `mvd_cache` with `pack8to16(mpx, mpy)` — the
    // motion-vector predictor — so this partition's predictor must be
    // computed before its context is known.
    let mvp = mvp_lx_parts(
        &pic.mbs,
        Some(m),
        map,
        p.x4,
        p.y4,
        p.w4,
        p.h4,
        if l == 0 { p.ref0 } else { p.ref1 },
        l,
    );
    // mvd context bases are per-COMPONENT: 40..46 for the x
    // component, 47..53 for y — the same pair for both lists
    // (spec Table 9-36 `mvd_lX` ranges; the reference decoder
    // calls decode_cabac_mb_mvd(40/47) regardless of list).
    let mut mvd = [0i16; 2];
    for (c, mv) in mvd.iter_mut().enumerate() {
        let amvd = amvd_ctx(&pic.mbs, m, map, p.x4, p.y4, l, c);
        let ctxbase = if c == 0 { 40 } else { 47 };
        let bits = cab.mvd(ctxbase, amvd)?;
        *mv = bits.value as i16;
    }
    // Cache the decoded magnitude for later neighbours (reference
    // `mvd_cache` = `*mvda` from `decode_cabac_mb_mvd`: |mvd| clipped
    // at 70 — NOT the predictor. `amvd_ctx` sums these directly).
    let cell = [
        mvd[0].unsigned_abs().min(70) as u8,
        mvd[1].unsigned_abs().min(70) as u8,
    ];
    // Final MV = mvp + mvd, committed per partition so later
    // same-MB partitions' predictors see it (reference `mv_cache`
    // is filled at decode time, not at write-back).
    let mv_final = [
        (i32::from(mvp[0]) + i32::from(mvd[0])).clamp(-8192, 8191) as i16,
        (i32::from(mvp[1]) + i32::from(mvd[1])).clamp(-8192, 8191) as i16,
    ];
    for yy in p.y4..p.y4 + p.h4 {
        for xx in p.x4..p.x4 + p.w4 {
            let b = block_index(xx, yy);
            if l == 0 {
                m.mvd_l0[b] = cell;
                m.mv[b] = mv_final;
                m.mv_valid[b] = true;
                // ref must be committed even when the stream omits it
                // (num_ref_idx_active == 1): neighbours' MVP reads the
                // reference index, and 0xff means "unavailable".
                m.ref_idx[b] = p.ref0;
            } else {
                m.mvd_l1[b] = cell;
                m.mv_l1[b] = mv_final;
                m.ref_idx_l1[b] = p.ref1;
            }
        }
    }
    if l == 0 {
        p.mvd0 = mvd;
    } else {
        p.mvd1 = mvd;
    }
    Ok(())
}

/// The `mvd` context magnitude `amvd` for component `c` of list `l`:
/// sums `abs(mvd)` on the partition's left edge blocks (A) and top
/// edge blocks (B), spec 9.3.3.1.1.7's `mvabs` accumulation (reference
/// `get_cabac_mb_mvd_ctx`: `a + b` then `(a+b+3)>>2` thresholds 0/1/2
/// encoded inside `cab.mvd`).
fn amvd_ctx(
    mbs: &[MbState],
    m: &MbState,
    map: MbMap,
    x4: usize,
    y4: usize,
    l: usize,
    c: usize,
) -> u32 {
    // Spec 9.3.3.1.1.7: `mvabs` sums |mvd| of the blocks immediately
    // left (A) and above (B) the partition's top-left corner — one
    // cell each, not the whole partition edge (reference
    // `mvd_cache[scan8[n]-1] + mvd_cache[scan8[n]-8]`). Direct and
    // skipped neighbours store zero mvd.
    let a = if x4 > 0 {
        let b = block_index(x4 - 1, y4);
        if l == 0 {
            m.mvd_l0[b][c]
        } else {
            m.mvd_l1[b][c]
        }
    } else if let Some(n) = map.mb_a().and_then(|i| mbs.get(i)) {
        if n.slice_id == map.sid {
            let b = block_index(3, y4);
            if l == 0 {
                n.mvd_l0[b][c]
            } else {
                n.mvd_l1[b][c]
            }
        } else {
            0
        }
    } else {
        0
    };
    let b_ = if y4 > 0 {
        let b = block_index(x4, y4 - 1);
        if l == 0 {
            m.mvd_l0[b][c]
        } else {
            m.mvd_l1[b][c]
        }
    } else if let Some(n) = map.mb_b().and_then(|i| mbs.get(i)) {
        if n.slice_id == map.sid {
            let b = block_index(x4, 3);
            if l == 0 {
                n.mvd_l0[b][c]
            } else {
                n.mvd_l1[b][c]
            }
        } else {
            0
        }
    } else {
        0
    };
    u32::from(a) + u32::from(b_)
}

/// `ref_idx`+`mvd` reads for B whole-MB types (16x16 and two-part):
/// spec order — all L0 refs, all L1 refs, all L0 mvds, all L1 mvds.
#[allow(clippy::too_many_arguments)]
fn cabac_read_refs_mvds(
    pic: &mut Pic,
    cab: &mut cabac::Cabac<'_>,
    cx: &SliceCx<'_>,
    map: MbMap,
    m: &mut MbState,
    ps: &mut [Part],
    nparts: usize,
) -> Result<()> {
    let _ = nparts;
    // All L0 ref reads; each commit lands before the next read
    // because ref-cache ctx is position-dependent (reference decoder
    // fills ref_cache per iteration).
    if cx.h.num_ref_idx_l0_active > 1 {
        for p in ps.iter_mut() {
            if p.dirs & 1 != 0 {
                let ctx_i = cabac_ref_ctx(&pic.mbs, m, map, p.x4, p.y4, 0);
                p.ref0 = cab.ref_idx(ctx_i)? as u8;
                if p.ref0 as usize >= cx.h.num_ref_idx_l0_active as usize {
                    return Err(Error::BadValue("ref_idx_l0 out of list"));
                }
                for yy in p.y4..p.y4 + p.h4 {
                    for xx in p.x4..p.x4 + p.w4 {
                        m.ref_idx[block_index(xx, yy)] = p.ref0;
                    }
                }
            }
        }
    }
    if cx.h.num_ref_idx_l1_active > 1 {
        for p in ps.iter_mut() {
            if p.dirs & 2 != 0 {
                let ctx_i = cabac_ref_ctx(&pic.mbs, m, map, p.x4, p.y4, 1);
                p.ref1 = cab.ref_idx(ctx_i)? as u8;
                if p.ref1 as usize >= cx.h.num_ref_idx_l1_active as usize {
                    return Err(Error::BadValue("ref_idx_l1 out of list"));
                }
                for yy in p.y4..p.y4 + p.h4 {
                    for xx in p.x4..p.x4 + p.w4 {
                        m.ref_idx_l1[block_index(xx, yy)] = p.ref1;
                    }
                }
            }
        }
    }
    // mvd reads: all L0 mvd for every partition first, then all L1 —
    // the spec's per-list syntax order (reference decoder's
    // `for(list){for(i){for(j){mvd}}}`).
    for l in 0..2usize {
        for p in ps.iter_mut() {
            if p.dirs & (1 << l) != 0 {
                cabac_read_mvd_one(pic, cab, cx, map, m, p, l)?;
            }
        }
    }
    Ok(())
}

/// `coded_block_flag` context for a *coefficient block* (non-DC cats):
/// neighbour TotalCoeff via the same nC machinery CAVLC uses.
fn nc_cbf_ctx(
    mbs: &[MbState],
    nz_acc: &[u8; 24],
    map: MbMap,
    blk: usize,
    plane: Plane,
    cur_intra: bool,
) -> usize {
    // ctxIdxInc = (nza > 0) + 2*(nzb > 0) where nz are the neighbour
    // block TotalCoeffs (spec 9.3.3.1.1.9). Chroma grids are 2x2.
    let (bx, by) = plane.xy(blk);
    let left_nc = |mbs: &[MbState]| -> i32 {
        if bx > 0 {
            i32::from(
                nz_acc[plane.nz_idx(match plane {
                    Plane::Luma => block_index(bx - 1, by),
                    _ => by * 2 + bx - 1,
                })],
            )
        } else if let Some(n) = map.mb_a().and_then(|i| mbs.get(i)) {
            if n.slice_id == map.sid {
                i32::from(
                    n.nz[plane.nz_idx(match plane {
                        Plane::Luma => block_index(3, by),
                        _ => by * 2 + 1,
                    })],
                )
            } else {
                -1
            }
        } else {
            -1
        }
    };
    let top_nc = |mbs: &[MbState]| -> i32 {
        if by > 0 {
            i32::from(
                nz_acc[plane.nz_idx(match plane {
                    Plane::Luma => block_index(bx, by - 1),
                    _ => (by - 1) * 2 + bx,
                })],
            )
        } else if let Some(n) = map.mb_b().and_then(|i| mbs.get(i)) {
            if n.slice_id == map.sid {
                i32::from(
                    n.nz[plane.nz_idx(match plane {
                        Plane::Luma => block_index(bx, 3),
                        _ => 2 + bx,
                    })],
                )
            } else {
                -1
            }
        } else {
            -1
        }
    };
    // Unavailable neighbours read as 64 coefficients for intra MBs
    // (reference `nnz_excluded`), 0 for inter — NOT always 0.
    let edge = if cur_intra { 64 } else { 0 };
    let a = {
        let v = left_nc(mbs);
        if v < 0 { edge } else { v }
    };
    let b = {
        let v = top_nc(mbs);
        if v < 0 { edge } else { v }
    };
    usize::from(a > 0) + 2 * usize::from(b > 0)
}

/// `coded_block_flag` context for a *DC* category (cat 0 luma DC and
/// cat 3 chroma DC): neighbours' `dc_coded` bits (spec 9.3.3.1.1.9
/// `coded_block_flag` ctxIdxInc — the neighbour "coded" bit is the
/// corresponding DC flag: left = A, top = B; `cat` selects which bit).
///
/// ffmpeg `get_cabac_cbf_ctx` (is_dc) reads `left_cbp`/`top_cbp` DC
/// bits (0x100 / 0x40 / 0x80). For a missing neighbour those default
/// to 0x7CF for an intra MB (all DC bits SET — each unavailable
/// neighbour contributes 1) and 0x00F for an inter MB (contributes 0),
/// so the default depends on the current MB's intra flag.
fn dc_cbf_ctx(mbs: &[MbState], map: MbMap, cat_off: usize, cur_intra: bool) -> usize {
    // cat_off: 0 = luma DC (dc_coded bit 0), 1 = Cb DC (bit 1),
    // 2 = Cr DC (bit 2).
    let bit = 1 << cat_off;
    let a = map
        .mb_a()
        .and_then(|i| mbs.get(i))
        .filter(|n| n.slice_id == map.sid)
        .map_or(u8::from(cur_intra), |n| u8::from(n.dc_coded & bit != 0));
    let b = map
        .mb_b()
        .and_then(|i| mbs.get(i))
        .filter(|n| n.slice_id == map.sid)
        .map_or(u8::from(cur_intra), |n| u8::from(n.dc_coded & bit != 0));
    usize::from(a) + 2 * usize::from(b)
}

/// `intra_chroma_pred_mode` CABAC context: neighbours with a coded
/// non-zero chroma mode (spec 9.3.2.6's ctxIdxInc counting
/// `intra_chroma_pred_mode != 0` neighbours; non-intra/unavailable
/// neighbours count as 0).
fn cabac_chroma_ctx(mbs: &[MbState], map: &MbMap) -> usize {
    let mut ctx = 0usize;
    for &n in &[map.mb_a(), map.mb_b()] {
        if let Some(m) = n.and_then(|i| mbs.get(i)) {
            if m.slice_id == map.sid && m.mb_type.is_intra() && m.chroma_pred != 0 {
                ctx += 1;
            }
        }
    }
    ctx.min(2)
}

/// MVP + MV store + MC for one coded partition (B or P).
#[allow(clippy::too_many_arguments)]
fn reconstruct_part(
    pic: &mut Pic,
    cx: &SliceCx<'_>,
    dpb: &Dpb,
    map: MbMap,
    m: &mut MbState,
    p: Part,
) -> Result<()> {
    let px0 = map.x * 16;
    let py0 = map.y * 16;
    // Use the parse-time MV, not a re-derivation: `cabac_read_mvd_one`
    // committed mvp+mvd into the motion state in decode order, which is
    // exactly what the reference decoder's mv_cache carries into
    // reconstruction. Recomputing here would see LATER same-MB
    // partitions already committed (different candidate availability)
    // and overwrite the correct MV.
    let b0 = block_index(p.x4, p.y4);
    let mut mv0 = [0i16; 2];
    let mut mv1 = [0i16; 2];
    let (mut r0, mut r1) = (-1i32, -1i32);
    if p.dirs & 1 != 0 {
        mv0 = m.mv[b0];
        r0 = i32::from(p.ref0);
    }
    if p.dirs & 2 != 0 {
        mv1 = m.mv_l1[b0];
        r1 = i32::from(p.ref1);
    }
    // Store per-4x4 motion state for neighbours + coloc + deblock.
    for yy in p.y4..p.y4 + p.h4 {
        for xx in p.x4..p.x4 + p.w4 {
            let b = block_index(xx, yy);
            if p.dirs & 1 != 0 {
                m.mv[b] = mv0;
                m.ref_idx[b] = p.ref0;
                m.mv_valid[b] = true;
            } else {
                m.ref_idx[b] = 0xff;
                m.mv[b] = [0, 0];
                m.mv_valid[b] = false;
            }
            if p.dirs & 2 != 0 {
                m.mv_l1[b] = mv1;
                m.ref_idx_l1[b] = p.ref1;
            } else {
                m.ref_idx_l1[b] = 0xff;
                m.mv_l1[b] = [0, 0];
            }
        }
    }
    mc_partition_bi(
        &mut pic.buf,
        cx,
        dpb,
        px0 + p.x4 * 4,
        py0 + p.y4 * 4,
        p.w4 * 4,
        p.h4 * 4,
        [i32::from(mv0[0]), i32::from(mv0[1])],
        [i32::from(mv1[0]), i32::from(mv1[1])],
        r0,
        r1,
    )
}

/// `mvp_l0_parts` generalised to either list: the same spec
/// 8.4.1.3.2 algorithm, with `mv_at_l` reading the L0 or L1 caches so
/// B's L1 partitions predict from L1 neighbours. Same-MB partitions
/// coded earlier join through `cur`.
#[allow(clippy::too_many_arguments)]
fn mvp_lx_parts(
    mbs: &[MbState],
    cur: Option<&MbState>,
    map: MbMap,
    x4: usize,
    y4: usize,
    w4: usize,
    h4: usize,
    ref_idx: u8,
    l: usize,
) -> [i16; 2] {
    let a = mv_at_l(mbs, cur, map, x4 as i32 - 1, y4 as i32, l);
    let b = mv_at_l(mbs, cur, map, x4 as i32, y4 as i32 - 1, l);
    let mut c = mv_at_l(mbs, cur, map, x4 as i32 + w4 as i32, y4 as i32 - 1, l);
    if c.is_none() {
        c = mv_at_l(mbs, cur, map, x4 as i32 - 1, y4 as i32 - 1, l);
    }
    let (mv_a, ref_a) = a.unwrap_or(([0, 0], -1));
    let (mv_b, ref_b) = b.unwrap_or(([0, 0], -1));
    let (mv_c, ref_c) = c.unwrap_or(([0, 0], -1));
    let ri = i32::from(ref_idx);
    // Directional single-candidate rules are MB-partition-only (spec
    // 8.4.1.1 "If MbPartWidth is 16 and MbPartHeight is 8..." / the
    // 8x16 analogue; ffmpeg keeps them in pred_16x8/pred_8x16_motion,
    // called only from MB-partition decode). Sub-mb partitions
    // (8x8/8x4/4x8/4x4) always take the median path — the previous
    // 8x4/4x8 cases generalised the rules down to sub-mbs and broke
    // every mixed-sub P8x8 MB in t2 (t2_8x8_part frame 1+).
    if w4 == 4 && h4 == 2 {
        if y4 == 0 && ref_b == ri {
            return mv_b;
        }
        if y4 == 2 && ref_a == ri {
            return mv_a;
        }
    }
    if w4 == 2 && h4 == 4 {
        if x4 == 0 && ref_a == ri {
            return mv_a;
        }
        if x4 == 2 && ref_c == ri {
            return mv_c;
        }
    }
    // Median (spec 8-211/8-212): a single matching ref short-circuits;
    // with B and C both unavailable the predictor is A alone.
    if b.is_none() && c.is_none() {
        return mv_a;
    }
    let matches = [ref_a == ri, ref_b == ri, ref_c == ri];
    if matches.iter().filter(|&&m| m).count() == 1 {
        if matches[0] {
            return mv_a;
        }
        if matches[1] {
            return mv_b;
        }
        return mv_c;
    }
    [
        med3(mv_a[0], mv_b[0], mv_c[0]),
        med3(mv_a[1], mv_b[1], mv_c[1]),
    ]
}

/// Neighbour samples for one Intra-8x8 group (spec 8.3.5.2's
/// availability, ported from the reference decoder's per-group
/// `topleft/topright_samples_available` handling). Groups are the
/// 8x8 quadrants in raster order (0..3); same-MB groups decoded
/// earlier are valid predictors.
#[allow(clippy::too_many_arguments)]
fn gather_luma8x8(buf: &FrameBuf, mbs: &[MbState], map: MbMap, g: usize, pps: &Pps) -> NbSamples {
    let gx = (g % 2) * 2;
    let gy = (g / 2) * 2;
    let px = map.x * 16 + gx * 4;
    let py = map.y * 16 + gy * 4;
    let mut nb = NbSamples {
        left: [0; 16],
        top: [0; 16],
        top_right: None,
        top_left: None,
        has_left: false,
        has_top: false,
    };
    let ok = |i: Option<usize>| -> bool {
        i.and_then(|j| mbs.get(j))
            .map(|s| intra_ok(s, pps, map.sid))
            .unwrap_or(false)
    };
    // LEFT column (px-1, py..py+7): group 1/3 read the previous
    // same-MB group (always available — intra MBs reconstruct in
    // group order); group 0/2 read mbA's right edge.
    if g % 2 == 1 || (px > 0 && ok(map.mb_a())) {
        nb.has_left = true;
        for i in 0..8 {
            nb.left[i] = buf.y[(py + i) * buf.w + px - 1];
        }
    }
    // TOP row (px..px+7, py-1): groups 2/3 read the same-MB group
    // above; groups 0/1 read mbB's bottom row.
    if g >= 2 || (py > 0 && ok(map.mb_b())) {
        nb.has_top = true;
        for i in 0..8 {
            nb.top[i] = buf.y[(py - 1) * buf.w + px + i];
        }
    }
    // TOP_LEFT sample (px-1, py-1): g0 -> mbD, g1 -> mbB, g2 -> mbA,
    // g3 -> the same-MB group-0 corner (already reconstructed).
    let tl: Option<u8> = match g {
        0 => {
            if px > 0 && py > 0 && ok(map.mb_d()) {
                Some(buf.y[(py - 1) * buf.w + px - 1])
            } else {
                None
            }
        }
        1 => {
            if py > 0 && ok(map.mb_b()) {
                Some(buf.y[(py - 1) * buf.w + px - 1])
            } else {
                None
            }
        }
        2 => {
            if px > 0 && ok(map.mb_a()) {
                Some(buf.y[(py - 1) * buf.w + px - 1])
            } else {
                None
            }
        }
        _ => Some(buf.y[(py - 1) * buf.w + px - 1]),
    };
    nb.top_left = tl;
    // TOP_RIGHT row (px+8..px+15, py-1): g0 -> mbB's right half;
    // g1 -> mbC's left half; g2 -> same-MB group-1's bottom row
    // (reconstructed already); g3 -> the not-yet-decoded right MB.
    nb.top_right = match g {
        0 => {
            if py > 0 && ok(map.mb_b()) {
                let mut t = [0u8; 16];
                for (i, tv) in t.iter_mut().enumerate().take(8) {
                    *tv = buf.y[(py - 1) * buf.w + px + 8 + i];
                }
                Some(t)
            } else {
                None
            }
        }
        1 => {
            if py > 0 && ok(map.mb_c()) {
                let mut t = [0u8; 16];
                for (i, tv) in t.iter_mut().enumerate().take(8) {
                    *tv = buf.y[(py - 1) * buf.w + px + 8 + i];
                }
                Some(t)
            } else {
                None
            }
        }
        2 => {
            let mut t = [0u8; 16];
            for (i, tv) in t.iter_mut().enumerate().take(8) {
                *tv = buf.y[(py - 1) * buf.w + px + 8 + i];
            }
            Some(t)
        }
        _ => None,
    };
    nb
}

/// Whether `transform_size_8x8_flag` is syntactically present for the
/// current sequence (spec 7.3.5 mb_pred + ffmpeg `dct8x8_allowed` =
/// `pps.transform_8x8_mode` — `direct_8x8_inference` only *narrows*
/// the inter gate; it never makes the flag appear on its own).
fn cabac_t8x8_allowed(cx: &SliceCx<'_>) -> bool {
    cx.pps.transform_8x8_mode
}

/// ctxIdxInc for `transform_size_8x8_flag` (spec 9.3.3.1.1.9 / Table
/// 9-11 ctxIdx 399): neighbour A/B `transform8x8` bits; missing or
/// non-8x8 neighbours count 0.
fn cabac_ctx399(mbs: &[MbState], map: &MbMap) -> usize {
    let f = |i: Option<usize>| -> usize {
        i.and_then(|j| mbs.get(j))
            .map(|m| usize::from(m.slice_id == map.sid && m.transform8x8))
            .unwrap_or(0)
    };
    f(map.mb_a()) + f(map.mb_b())
}
