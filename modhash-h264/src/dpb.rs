//! Decoded picture buffer: reference marking (sliding window + MMCO
//! ops 1–4), PicNum derivation, and RefPicList0 initialisation plus
//! slice reordering (spec 8.2.4).
//!
//! Only *short-term* frames are implemented — baseline encoders
//! (x264/openh264) never emit long-term marking, and the slice parser
//! already refuses `long_term_reference_flag` on IDR. MMCO op 3/5/6
//! (long-term assigns) surface as [`Error::Unsupported`] rather than
//! silently marking wrong.

use crate::slice::SliceHeader;
use alloc::vec::Vec;
use modhash_primitives::{Error, Result};

/// One reference picture in the DPB: planes + PicNum bookkeeping.
#[derive(Clone, Debug)]
pub(crate) struct RefFrame {
    /// `frame_num` the picture was coded with.
    pub frame_num: u32,
    /// `PicNum` of the *current* decode pass: `FrameNum` adjusted by
    /// `FrameNumWrap` (spec 8.2.4.1); recomputed per slice.
    pub pic_num: u32,
    /// Luma plane (coded width × coded height, row-major).
    pub y: Vec<u8>,
    /// Cb plane.
    pub cb: Vec<u8>,
    /// Cr plane.
    pub cr: Vec<u8>,
}

/// The DPB: `refs` is the set of short-term reference frames plus the
/// sliding-window/marked bookkeeping.
#[derive(Default)]
pub(crate) struct Dpb {
    /// Short-term reference frames, insertion order = decode order.
    pub refs: Vec<RefFrame>,
    /// `MaxFrameNum` = `1 << log2_max_frame_num` of the active SPS.
    pub max_frame_num: u32,
    /// `max_num_ref_frames` honoured (min of SPS and Limits).
    pub max_refs: usize,
}

impl Dpb {
    /// DPB state for a new SPS (or IDR): everything is dropped, sizes
    /// updated.
    pub(crate) fn reset(&mut self, max_frame_num: u32, max_refs: u32, limit: u32) {
        self.refs.clear();
        self.max_frame_num = max_frame_num.max(1);
        self.max_refs = (max_refs.min(limit).max(1)) as usize;
    }

    /// IDR boundary: mark every reference unused-for-reference before
    /// the new picture (spec 8.2.5.2; `no_output_of_prior_pics`/`long
    /// term_reference_flag` variations don't apply — we reject the
    /// long-term flag at parse and output doesn't interleave with DPB).
    pub(crate) fn flush(&mut self) {
        self.refs.clear();
    }

    /// Recomputes `PicNum` for every stored reference against the
    /// current picture's `frame_num` (spec 8.2.4.1: PicNum = frame_num
    /// − MaxFrameNum when frame_num > FrameNum, else frame_num).
    fn set_pic_nums(&mut self, frame_num: u32) {
        for r in self.refs.iter_mut() {
            r.pic_num = if r.frame_num > frame_num {
                r.frame_num.wrapping_sub(self.max_frame_num)
            } else {
                r.frame_num
            };
        }
    }

    /// Applies this slice's reference-picture marking after the picture
    /// is decoded. `cur` is the just-finished frame (already in `refs`
    /// via [`push`]).
    pub(crate) fn apply_marking(
        &mut self,
        h: &SliceHeader,
        frame_num: u32,
        nal_ref_idc: u8,
    ) -> Result<()> {
        if nal_ref_idc == 0 {
            return Ok(());
        }
        if h.adaptive_marking {
            for m in &h.mmco {
                match m.op {
                    // mark_short_term_unused: PicNum = FrameNum − diff
                    // (picNumX per 8.2.5.4.1).
                    1 => {
                        let target = (i64::from(frame_num) - i64::from(m.difference_of_pic_nums))
                            .rem_euclid(i64::from(self.max_frame_num))
                            as u32;
                        self.refs.retain(|r| r.pic_num != target);
                    }
                    2 => return Err(Error::Unsupported("h264 MMCO long-term pic mark")),
                    3 | 6 => return Err(Error::Unsupported("h264 long-term reference marking")),
                    4 => return Err(Error::Unsupported("h264 MMCO max_long_term_frame_idx")),
                    // 5: reset — all refs unused + POC counters; our
                    // sliding window only needs the clear.
                    5 => self.refs.clear(),
                    _ => return Err(Error::BadValue("mmco op out of range")),
                }
            }
        }
        // Sliding window (spec 8.2.5.3): when adaptive marking did not
        // run, evict the lowest PicNum until within max_refs.
        if !h.adaptive_marking {
            while self.refs.len() > self.max_refs {
                // PicNums are all <= frame_num; evict the smallest.
                let mut lowest = 0usize;
                for (i, r) in self.refs.iter().enumerate() {
                    if r.pic_num < self.refs[lowest].pic_num {
                        lowest = i;
                    }
                }
                self.refs.remove(lowest);
            }
        }
        Ok(())
    }

    /// Inserts the finished frame as a reference (caller skips when
    /// `nal_ref_idc == 0`) and returns its index.
    pub(crate) fn push(&mut self, frame_num: u32, y: Vec<u8>, cb: Vec<u8>, cr: Vec<u8>) -> usize {
        self.refs.push(RefFrame {
            frame_num,
            pic_num: frame_num,
            y,
            cb,
            cr,
        });
        self.refs.len() - 1
    }

    /// Builds the initial RefPicList0 for a P or B slice and applies
    /// the slice's reordering commands (spec 8.2.4.2 + 8.2.4.3.1).
    ///
    /// Returns indices into `self.refs` in list order.
    pub(crate) fn ref_list0(&mut self, h: &SliceHeader, frame_num: u32) -> Result<Vec<usize>> {
        self.set_pic_nums(frame_num);
        // Initial order: PicNum descending (short-term only).
        let mut order: Vec<usize> = (0..self.refs.len()).collect();
        order.sort_by(|&a, &b| self.refs[b].pic_num.cmp(&self.refs[a].pic_num));
        self.apply_reorder(&mut order, &h.reorder_l0, frame_num)?;
        let cap = h.num_ref_idx_l0_active as usize;
        if order.len() > cap {
            order.truncate(cap);
        }
        Ok(order)
    }

    /// Builds the initial RefPicList1 for a B slice (spec 8.2.4.2.3):
    /// short-term frames in *increasing* PicNum order, then the L1
    /// reordering commands. Long-term refs are refused elsewhere.
    pub(crate) fn ref_list1(&mut self, h: &SliceHeader, frame_num: u32) -> Result<Vec<usize>> {
        self.set_pic_nums(frame_num);
        let mut order: Vec<usize> = (0..self.refs.len()).collect();
        order.sort_by(|&a, &b| self.refs[a].pic_num.cmp(&self.refs[b].pic_num));
        self.apply_reorder(&mut order, &h.reorder_l1, frame_num)?;
        let cap = h.num_ref_idx_l1_active as usize;
        if order.len() > cap {
            order.truncate(cap);
        }
        Ok(order)
    }

    /// Reference-picture-list reordering (spec 8.2.4.3.1), shared by
    /// L0 and L1: `pred` starts at the current picture's PicNum and
    /// each `abs_diff_pic_num_minus1` command walks it while the
    /// picked entry is moved to `refIdxLX`.
    fn apply_reorder(
        &mut self,
        order: &mut Vec<usize>,
        cmds: &[(u32, u32)],
        frame_num: u32,
    ) -> Result<()> {
        if cmds.is_empty() {
            return Ok(());
        }
        let n = order.len();
        let mut list = order.clone();
        let mut pred = i64::from(frame_num);
        let mut idx: usize = 0;
        for &(idc, val) in cmds {
            if idc == 0 || idc == 1 {
                let v = i64::from(val) + 1;
                pred = if idc == 0 {
                    (pred - v).rem_euclid(i64::from(self.max_frame_num))
                } else {
                    (pred + v).rem_euclid(i64::from(self.max_frame_num))
                };
                let want = pred as u32;
                let pos = list
                    .iter()
                    .position(|&i| self.refs[i].pic_num == want)
                    .ok_or(Error::BadValue("ref reorder target not in list"))?;
                if idx >= n.max(1) {
                    return Err(Error::BadValue("ref reorder index out of range"));
                }
                let r = list.remove(pos);
                let ins = idx.min(list.len());
                list.insert(ins, r);
                idx += 1;
            } else {
                return Err(Error::Unsupported("h264 long-term ref reorder"));
            }
        }
        *order = list;
        Ok(())
    }
}
