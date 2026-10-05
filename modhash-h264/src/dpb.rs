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

/// Colocation data of one macroblock, kept for temporal direct
/// prediction (spec 8.4.1.2): the colocated picture's L0/L1 motion
/// vectors and reference indices at 4x4-block granularity.
#[derive(Clone, Debug)]
pub(crate) struct ColocMb {
    /// L0 motion vectors per 4x4 block (group-major).
    pub mv_l0: [[i16; 2]; 16],
    /// L1 motion vectors per 4x4 block.
    pub mv_l1: [[i16; 2]; 16],
    /// L0 reference index of each 8x8 group's top-left 4x4 block
    /// (`0xff` = this list unused) — the granularity the direct-
    /// prediction colocation rules read (reference `ref_index[]`).
    pub ref_l0: [u8; 4],
    /// L1 reference indices per 8x8 group.
    pub ref_l1: [u8; 4],
    /// `true` when the colocated MB was intra (mv/ref ignored, direct
    /// prediction falls back to zeros).
    pub intra: bool,
}

/// One reference picture in the DPB: planes + PicNum/POC bookkeeping
/// and the colocation data B-slice direct prediction needs.
#[derive(Clone, Debug)]
pub(crate) struct RefFrame {
    /// `frame_num` the picture was coded with.
    pub frame_num: u32,
    /// `PicNum` of the *current* decode pass: `FrameNum` adjusted by
    /// `FrameNumWrap` (spec 8.2.4.1); recomputed per slice.
    pub pic_num: u32,
    /// Picture order count (spec 8.2.1): RefPicList1 ordering,
    /// temporal-direct `DistScaleFactor` and implicit weighted
    /// prediction all key on POC, not `frame_num`.
    pub poc: i64,
    /// Luma plane (coded width × coded height, row-major).
    pub y: Vec<u8>,
    /// Cb plane.
    pub cb: Vec<u8>,
    /// Cr plane.
    pub cr: Vec<u8>,
    /// Colocation data per macroblock (empty for pictures decoded
    /// before the first B slice appears — temporal direct tolerates
    /// it via the intra/zero path).
    pub coloc: Vec<ColocMb>,
    /// `frame_num`s of this picture's own RefPicList0 at decode time
    /// — the `map_col_to_list0` indirection temporal direct uses to
    /// resolve `refIdxCol` (spec 8.2.5.4 note: the colocated picture's
    /// reference lists are not stored, only their identities).
    pub ref_l0_fns: Vec<u32>,
    /// Same for RefPicList1.
    pub ref_l1_fns: Vec<u32>,
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
                    // 3: adaptive sliding window (8.2.5.3): evict the
                    // short-term ref with the smallest PicNum.
                    3 => {
                        if !self.refs.is_empty() {
                            let mut lowest = 0usize;
                            for (i, r) in self.refs.iter().enumerate() {
                                if r.pic_num < self.refs[lowest].pic_num {
                                    lowest = i;
                                }
                            }
                            self.refs.remove(lowest);
                        }
                    }
                    6 => return Err(Error::Unsupported("h264 long-term reference marking")),
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push(
        &mut self,
        frame_num: u32,
        poc: i64,
        y: Vec<u8>,
        cb: Vec<u8>,
        cr: Vec<u8>,
        coloc: Vec<ColocMb>,
        ref_l0_fns: Vec<u32>,
        ref_l1_fns: Vec<u32>,
    ) -> usize {
        self.refs.push(RefFrame {
            frame_num,
            pic_num: frame_num,
            poc,
            y,
            cb,
            cr,
            coloc,
            ref_l0_fns,
            ref_l1_fns,
        });
        self.refs.len() - 1
    }

    /// Builds the initial RefPicList0 for a P or B slice and applies
    /// the slice's reordering commands (spec 8.2.4.2 + 8.2.4.3.1).
    ///
    /// Returns indices into `self.refs` in list order.
    pub(crate) fn ref_list0(
        &mut self,
        h: &SliceHeader,
        frame_num: u32,
        cur_poc: i64,
        is_b: bool,
    ) -> Result<Vec<usize>> {
        self.set_pic_nums(frame_num);
        let mut order: Vec<usize> = (0..self.refs.len()).collect();
        if is_b {
            // B slices (spec 8.2.4.2.3): short-term refs with
            // poc < cur_poc first in *decreasing* POC, then
            // poc > cur_poc in *increasing* POC.
            order.sort_by(|&a, &b| {
                let (pa, pb) = (self.refs[a].poc, self.refs[b].poc);
                let (ba, bb) = (pa < cur_poc, pb < cur_poc);
                match (ba, bb) {
                    (true, true) => pb.cmp(&pa),
                    (false, false) => pa.cmp(&pb),
                    (true, false) => core::cmp::Ordering::Less,
                    (false, true) => core::cmp::Ordering::Greater,
                }
            });
        } else {
            // Initial order: PicNum descending (short-term only).
            order.sort_by(|&a, &b| self.refs[b].pic_num.cmp(&self.refs[a].pic_num));
        }
        // The reorder machinery works on `num_ref_idx_active` slots —
        // when the DPB holds fewer pictures the tail slots repeat the
        // last entry (spec 8.2.4.1's fill rule, as the reference
        // decoder's `default_ref` does), and a command targeting a
        // slot index >= cap is malformed.
        let cap = h.num_ref_idx_l0_active as usize;
        if cap == 0 {
            return Err(Error::BadValue("num_ref_idx_l0_active zero"));
        }
        while order.len() < cap {
            let last = *order.last().unwrap_or(&0);
            order.push(last);
        }
        // ffmpeg's ref list is always exactly `num_ref_idx_active`
        // entries; the reorder shift window must not see the tail.
        order.truncate(cap);
        self.apply_reorder(&mut order, &h.reorder_l0, frame_num, cap)?;
        Ok(order)
    }

    /// Builds the initial RefPicList1 for a B slice (spec 8.2.4.2.3):
    /// refs with poc > cur_poc first in increasing POC, then
    /// poc <= cur_poc in decreasing POC — the mirror image of L0 —
    /// followed by the L1 reordering commands and the mandated
    /// "if entry 0 equals L0[0], swap entries 0 and 1" rule.
    pub(crate) fn ref_list1(
        &mut self,
        h: &SliceHeader,
        frame_num: u32,
        cur_poc: i64,
        l0: &[usize],
    ) -> Result<Vec<usize>> {
        self.set_pic_nums(frame_num);
        let mut order: Vec<usize> = (0..self.refs.len()).collect();
        order.sort_by(|&a, &b| {
            let (pa, pb) = (self.refs[a].poc, self.refs[b].poc);
            // After-cur first (ascending), then before-or-equal
            // (descending).
            let (aa, ab) = (pa > cur_poc, pb > cur_poc);
            match (aa, ab) {
                (true, true) => pa.cmp(&pb),
                (false, false) => pb.cmp(&pa),
                (true, false) => core::cmp::Ordering::Less,
                (false, true) => core::cmp::Ordering::Greater,
            }
        });
        let cap = h.num_ref_idx_l1_active as usize;
        if cap == 0 {
            return Err(Error::BadValue("num_ref_idx_l1_active zero"));
        }
        while order.len() < cap {
            let last = *order.last().unwrap_or(&0);
            order.push(last);
        }
        order.truncate(cap);
        self.apply_reorder(&mut order, &h.reorder_l1, frame_num, cap)?;
        // Spec 8.2.4.2.3: when RefPicList1[0] == RefPicList0[0] and
        // both lists have entries, swap the first two L1 entries so
        // a same-picture pair does not stall direct prediction.
        if order.len() > 1 && !l0.is_empty() && order[0] == l0[0] {
            order.swap(0, 1);
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
        cap: usize,
    ) -> Result<()> {
        if cmds.is_empty() {
            return Ok(());
        }
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
                // ffmpeg `h264_refs.c` reorder semantics (the reference
                // decoder): resolve the target picture by PicNum among
                // all refs (any list position), then search only the
                // TAIL `list[idx..n-1)` for it. Found: shift the tail
                // right and place it at `idx` (moves it forward). Not
                // found: still shift + place — the picture lands at
                // `idx` even when it already sits earlier in the list
                // (a deliberate duplicate; remove+insert across the
                // whole list is WRONG and picks the wrong reference).
                let resolved = self
                    .refs
                    .iter()
                    .position(|r| r.pic_num == want)
                    .ok_or(Error::BadValue("ref reorder target not in list"))?;
                if idx >= cap {
                    return Err(Error::BadValue("ref reorder index out of range"));
                }
                let n = list.len();
                let mut pos = idx;
                let mut found = false;
                while pos + 1 < n {
                    if self.refs[list[pos]].pic_num == want {
                        found = true;
                        break;
                    }
                    pos += 1;
                }
                if !found {
                    pos = n.saturating_sub(1).max(idx);
                }
                for j in (idx + 1..=pos).rev() {
                    list[j] = list[j - 1];
                }
                list[idx] = resolved;
                idx += 1;
            } else {
                return Err(Error::Unsupported("h264 long-term ref reorder"));
            }
        }
        *order = list;
        Ok(())
    }
}
