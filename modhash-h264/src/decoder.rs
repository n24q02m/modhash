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
#[cfg(test)]
#[macro_export]
macro_rules! dbgln {
    ($($a:tt)*) => {{ extern crate std; std::eprintln!($($a)*) }};
}
#[cfg(not(test))]
macro_rules! dbgln {
    ($($a:tt)*) => {{}};
}

use crate::cavlc::{self, BlockKind};
use crate::deblock;
use crate::dpb::Dpb;
use crate::golomb::Br;
use crate::inter;
use crate::intra::{self, NbSamples};
use crate::mb::{MbMap, MbState, MbType, Nb, SubMbType, intra4x4_neighbours};
use crate::nal::{self, Nal};
use crate::pps::{self, Pps};
use crate::slice::{self, Mmco, SliceHeader, SliceType};
use crate::sps::{self, Sps};
use crate::tables::{CBP_INTER, CBP_INTRA, QPC_TABLE, block_index, block_xy};
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
    /// Monotonic slice counter feeding `MbState::slice_id`.
    slice_seq: u32,
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
    /// Slice id for `MbState::slice_id`.
    slice_id: u32,
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
            slice_seq: 0,
        }
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
            self.pic = Some(Pic {
                buf: FrameBuf::new(w, hh),
                mbs: alloc::vec![MbState::new(); (sps.width_mbs * sps.height_mbs) as usize],
                sps_id: sps.id,
                pps_id: h.pps_id,
                chroma_off_cb: pps.chroma_qp_index_offset,
                chroma_off_cr: pps.chroma_qp_index_offset_cr,
                frame_num: h.frame_num,
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
        let ref_order: Vec<usize> = if h.slice_type != SliceType::I {
            let order = self.dpb.ref_list0(&h, h.frame_num)?;
            if order.is_empty() {
                return Err(Error::BadValue("P/B slice with empty L0 list"));
            }
            order
        } else {
            Vec::new()
        };
        let ref_order1: Vec<usize> = if h.slice_type == SliceType::B {
            let order = self.dpb.ref_list1(&h, h.frame_num)?;
            if order.is_empty() {
                return Err(Error::BadValue("B slice with empty L1 list"));
            }
            order
        } else {
            Vec::new()
        };
        let mut cx = SliceCx {
            sps: &sps,
            pps: &pps,
            h: &h,
            ref_order: &ref_order,
            ref_order1: &ref_order1,
            qp_prev: pps.pic_init_qp + h.slice_qp_delta,
            slice_id,
        };
        let pic = self.pic.as_mut().unwrap();
        slice_data(pic, &mut br, &mut cx, &self.dpb)
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
        dbgln!("pre-deblock y[0..16] {:?}", &buf.y[0..16]);
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
        dbgln!("post-deblock y[0..16] {:?}", &buf.y[0..16]);
        if pic.nal_ref_idc != 0 {
            self.dpb
                .push(pic.frame_num, buf.y.clone(), buf.cb.clone(), buf.cr.clone());
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
        }
        if self.frames.len() >= self.limits.max_frames {
            return Err(Error::too_large("h264 frames", self.limits.max_frames));
        }
        self.frames.push(crop_frame(&sps, &buf)?);
        Ok(())
    }

    /// End-of-stream: flushes the in-progress picture (if any) and
    /// returns every decoded frame in presentation order.
    /// debug
    fn end_of_stream(&mut self) -> Result<Vec<Frame>> {
        self.finish_picture()?;
        Ok(core::mem::take(&mut self.frames))
    }
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

/// The macroblock-layer loop of `slice_data` (spec 7.3.4).
fn slice_data(pic: &mut Pic, br: &mut Br<'_>, cx: &mut SliceCx<'_>, dpb: &Dpb) -> Result<()> {
    let wm = cx.sps.width_mbs as usize;
    let total = (cx.sps.width_mbs * cx.sps.height_mbs) as usize;
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

/// P_Skip reconstruction (spec 8.4.1 / 7.3.5.2): one 16x16 partition,
/// `ref_idx` 0, MV = the median predictor, no residual.
fn p_skip(pic: &mut Pic, cx: &SliceCx<'_>, dpb: &Dpb, idx: usize, wm: usize) -> Result<()> {
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
    dbgln!("  skip mb{idx} mvp {mvp:?}");
    {
        let m = &mut pic.mbs[idx];
        m.mb_type = MbType::PSkip;
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
fn b_skip(pic: &mut Pic, cx: &SliceCx<'_>, dpb: &Dpb, idx: usize, wm: usize) -> Result<()> {
    let map = MbMap {
        idx,
        x: idx % wm,
        y: idx / wm,
        width: wm,
        sid: cx.slice_id,
    };
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
    }
    direct_motion(pic, cx, dpb, idx, wm)?;
    Ok(())
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
    dbgln!("mb {idx} type_code {code} pos {}", br.position());
    let mb_type = match cx.h.slice_type {
        SliceType::I => MbType::i_slice(code)?,
        SliceType::P => MbType::p_slice(code)?,
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

    // ---- mb_pred / sub_mb_pred (spec 7.3.5) ----
    // Inter partitions as `(x4, y4, w4, h4, ref_idx, mvd)` on the
    // 4x4-block grid.
    let mut inter_parts: Vec<(usize, usize, usize, usize, u8, [i16; 2])> = Vec::new();
    match mb_type {
        MbType::I4x4 => {
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
                dbgln!("  blk {blk} r{r} mpm {mpm} flag {flag} -> mode {mode}");
                m.i4x4_modes[r] = mode;
            }
            m.chroma_pred = br.ue()? as u8;
            dbgln!("  chroma_pred {} pos {}", m.chroma_pred, br.position());
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
            // ref_idx_l0 coded once per 8x8 sub-mb (spec 7.3.5.1);
            // P_8x8ref0 implies zero.
            let mut refs = [0u8; 4];
            let need_ref = cx.h.num_ref_idx_l0_active > 1 && mb_type != MbType::P8x8Ref0;
            for r in refs.iter_mut() {
                if need_ref {
                    *r = br.te(cx.h.num_ref_idx_l0_active - 1)? as u8;
                    if *r as usize >= cx.ref_order.len() {
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
                    if *r as usize >= cx.ref_order.len() {
                        return Err(Error::BadValue("ref_idx_l0 out of list"));
                    }
                }
            }
            for (pi, &(x4, y4, w4, h4)) in parts.iter().take(nparts).enumerate() {
                let mvd = clamp_mvd([br.se()?, br.se()?]);
                dbgln!(
                    "  part {x4},{y4} {w4}x{h4} ref{} mvd{mvd:?} pos {}",
                    refs[pi],
                    br.position()
                );
                inter_parts.push((x4, y4, w4, h4, refs[pi], mvd));
            }
        }
    }

    // ---- coded_block_pattern + mb_qp_delta (spec 7.3.5) ----
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
            dbgln!(
                "  cbp code {cbp} -> luma {cbp_luma} chroma {cbp_chroma} pos {}",
                br.position()
            );
        }
    }
    let coded = cbp_luma > 0 || cbp_chroma > 0 || matches!(mb_type, MbType::I16x16 { .. });
    let mut qp_y = cx.qp_prev;
    if coded && mb_type != MbType::IPcm {
        let delta = br.se()?;
        dbgln!("  qpd {delta} pos {}", br.position());
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
    let mut dc_y = [0i32; 16];
    let mut chroma_ac = [[[0i32; 16]; 4]; 2];
    let mut chroma_dc = [[0i32; 4]; 2];
    if mb_type != MbType::IPcm {
        if let MbType::I16x16 { .. } = mb_type {
            let nc = nc_value(&pic.mbs, &nz_acc, map, 0, Plane::Luma);
            let r = cavlc::decode_block(br, nc, BlockKind::LumaOrChromaAc4x4)?;
            dbgln!("  i16dcr tc {} levels {:?}", r.total_coeff, &r.levels);
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
            for g in 0..4usize {
                if cbp_luma & (1 << g) == 0 {
                    continue;
                }
                for s4 in 0..4usize {
                    let blk = g * 4 + s4;
                    let nc = nc_value(&pic.mbs, &nz_acc, map, blk, Plane::Luma);
                    let r = cavlc::decode_block(br, nc, BlockKind::LumaOrChromaAc4x4)?;
                    dbgln!(
                        "  cavlc blk {blk} nc {nc} tc {} pos {} lv {:?}",
                        r.total_coeff,
                        br.position(),
                        &r.levels[..8]
                    );
                    for s in 0..16 {
                        let ri = cavlc::raster_index(BlockKind::LumaOrChromaAc4x4, s);
                        luma_res[blk][ri] = r.levels[s];
                    }
                    nz_acc[blk] = r.total_coeff;
                }
            }
        }
        if cbp_chroma > 0 {
            for dc in chroma_dc.iter_mut() {
                let r = cavlc::decode_block(br, -1, BlockKind::ChromaDc)?;
                dbgln!("  chromaDC tc{} pos{}", r.total_coeff, br.position());
                for s in 0..4 {
                    dc[cavlc::raster_index(BlockKind::ChromaDc, s)] = r.levels[s];
                }
            }
            if cbp_chroma == 2 {
                for c in 0..2usize {
                    for b in 0..4usize {
                        let nc = nc_value(&pic.mbs, &nz_acc, map, b, Plane::chroma(c));
                        let r = cavlc::decode_block(br, nc, BlockKind::Ac15)?;
                        dbgln!(
                            "  chromaAC c{c} b{b} nc{nc} tc{} pos{}",
                            r.total_coeff,
                            br.position()
                        );
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
    dbgln!(
        "  mb{idx} nz luma {:?} cb {:?} cr {:?}",
        &nz_acc[..16],
        &nz_acc[16..20],
        &nz_acc[20..24]
    );

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
            // Reconstruct in group-major coding order.
            for (blk, raw_resid) in luma_res.iter().enumerate() {
                let (bx, by) = block_xy(blk);
                let r = by * 4 + bx;
                let nb = gather4x4(&pic.buf, &pic.mbs, map, bx, by, blk, cx.pps);
                let mut pred = [0u8; 16];
                intra::pred4x4(m.i4x4_modes[r], &nb, &mut pred)?;
                dbgln!(
                    "  recon2 blk {blk} mode {} l{} t{} {:?} resid {:?}",
                    m.i4x4_modes[r],
                    nb.has_left,
                    nb.has_top,
                    &pred,
                    raw_resid
                );
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
            dbgln!("  i16dcr hadamard {:?} qp {}", &dc, m.qp_y);
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
                if blk == 0 {
                    dbgln!("  i16 blk0 resid {:?}", &resid);
                }
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
            // Inter: per partition MVP + MC, then residual add.
            for &(x4, y4, w4, h4, ref_idx, mvd) in &inter_parts {
                let mvp = mvp_l0_parts(&pic.mbs, Some(&m), map, x4, y4, w4, h4, ref_idx);
                let mv = [
                    (i32::from(mvp[0]) + i32::from(mvd[0])).clamp(-8192, 8191) as i16,
                    (i32::from(mvp[1]) + i32::from(mvd[1])).clamp(-8192, 8191) as i16,
                ];
                dbgln!("    part {x4},{y4} mvp {mvp:?} -> mv {mv:?} ref{ref_idx}");
                for yy in y4..y4 + h4 {
                    for xx in x4..x4 + w4 {
                        let b = block_index(xx, yy);
                        m.mv[b] = mv;
                        m.ref_idx[b] = ref_idx;
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
            // Add luma residual where cbp says coefficients exist.
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
        dbgln!(
            "  chroma{c} dc raw {:?} -> {:?} qpc {qpc}",
            chroma_dc[c],
            dc
        );
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
    if let Some(wp) = &cx.h.wp_l0 {
        if let Some(e) = wp.get(ri) {
            if e.luma_flag {
                inter::apply_wp(&mut luma, e.luma.0, e.luma.1, cx.h.wp_denom.0);
            }
        }
    }
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
            dbgln!(
                "      nc fetch ({nx},{ny}) blk{blk} foreign mb{nidx} UNAVAIL sid{}",
                nb.slice_id
            );
            return None;
        }
        if nb.mb_type == MbType::IPcm {
            return Some(16);
        }
        let lx = (nx + dim) % dim;
        let ly = (ny + dim) % dim;
        let v = i32::from(nb.nz[plane.nz_idx(plane.index(lx as usize, ly as usize))]);
        dbgln!("      nc fetch ({nx},{ny}) blk{blk} mb{nidx} nz[{lx},{ly}]={v}");
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
    // Directional cases (spec 8-203..8-206 + sub-mb analogues).
    if w4 * 4 == 16 && h4 * 4 == 8 {
        if y4 == 0 && ref_b == ri {
            return mv_b;
        }
        if y4 == 2 && ref_a == ri {
            return mv_a;
        }
    }
    if w4 * 4 == 8 && h4 * 4 == 16 {
        if x4 == 0 && ref_a == ri {
            return mv_a;
        }
        if x4 == 2 && ref_c == ri {
            return mv_c;
        }
    }
    if w4 * 4 == 8 && h4 * 4 == 4 {
        if y4 % 2 == 0 && ref_b == ri {
            return mv_b;
        }
        if y4 % 2 == 1 && ref_a == ri {
            return mv_a;
        }
    }
    if w4 * 4 == 4 && h4 * 4 == 8 {
        if x4 % 2 == 0 && ref_a == ri {
            return mv_a;
        }
        if x4 % 2 == 1 && ref_c == ri {
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
        return Some((nb.mv[b], if r == 0xff { -1 } else { i32::from(r) }));
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
        match crate::decode(&s) {
            Ok(f) => std::eprintln!("decoded {} frames", f.len()),
            Err(e) => std::eprintln!("ERR {e:?}"),
        }
    }
}
