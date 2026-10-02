//! Intra prediction (spec 8.3): Intra-4x4's nine modes,
//! Intra-16x16's four, and the 8x8 chroma modes. All functions are
//! pure: the caller gathers the neighbouring samples (with their
//! availability flags) and every function here is a closed-form
//! expression over them.

use modhash_primitives::{Error, Result};

/// The neighbour samples one 4x4 (or 8x8/16x16) block's predictor is
/// built from. `top_right` is only fetched for modes that need it.
#[derive(Clone, Debug)]
pub(crate) struct NbSamples {
    /// `p[-1, 0..n)` left column (n = block size).
    pub left: [u8; 16],
    /// `p[0..n, -1]` top row.
    pub top: [u8; 16],
    /// `p[n..2n, -1]` top-right row continuation, when available.
    pub top_right: Option<[u8; 16]>,
    /// `p[-1,-1]` top-left corner, when available.
    pub top_left: Option<u8>,
    /// Left column available.
    pub has_left: bool,
    /// Top row available.
    pub has_top: bool,
}

fn clip(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

/// Predicted `Intra4x4PredMode` for a block (spec 8.3.1.1): the minimum
/// of the neighbours' modes when both are intra-4x4, else DC (2).
pub(crate) fn most_probable(mode_a: Option<u8>, mode_b: Option<u8>) -> u8 {
    match (mode_a, mode_b) {
        (Some(a), Some(b)) => a.min(b),
        _ => 2,
    }
}

/// The nine Intra-4x4 modes (spec 8.3.1.2) over 16 output samples in
/// raster order. `mode` must be < 9; `Err` otherwise.
pub(crate) fn pred4x4(mode: u8, nb: &NbSamples, out: &mut [u8; 16]) -> Result<()> {
    let t = &nb.top[..4];
    let l = &nb.left[..4];
    let tl = nb.top_left;
    match mode {
        // Intra_4x4_Vertical: needs the top row.
        0 => {
            if !nb.has_top {
                return Err(Error::BadValue("intra4x4 vertical without top"));
            }
            for y in 0..4 {
                out[y * 4..y * 4 + 4].copy_from_slice(t);
            }
        }
        // Horizontal: needs the left column.
        1 => {
            if !nb.has_left {
                return Err(Error::BadValue("intra4x4 horizontal without left"));
            }
            for y in 0..4 {
                for x in 0..4 {
                    out[y * 4 + x] = l[y];
                }
            }
        }
        // DC.
        2 => {
            let v = match (nb.has_left, nb.has_top) {
                (true, true) => {
                    (t.iter().map(|&s| u32::from(s)).sum::<u32>()
                        + l.iter().map(|&s| u32::from(s)).sum::<u32>()
                        + 4)
                        >> 3
                }
                (true, false) => (l.iter().map(|&s| u32::from(s)).sum::<u32>() + 2) >> 2,
                (false, true) => (t.iter().map(|&s| u32::from(s)).sum::<u32>() + 2) >> 2,
                (false, false) => 128,
            } as u8;
            out.fill(v);
        }
        // Diagonal_Down_Left: top + top-right.
        3 => {
            if !nb.has_top {
                return Err(Error::BadValue("intra4x4 diag_dl without top"));
            }
            let tr = match &nb.top_right {
                Some(tr) => *tr,
                // When p[4..8) is unavailable every one of them is
                // replaced by p[3,-1] (spec 8.3.1.2.1).
                None => {
                    let mut a = [0u8; 16];
                    a[..4].fill(t[3]);
                    a
                }
            };
            let p: [u8; 8] = [t[0], t[1], t[2], t[3], tr[0], tr[1], tr[2], tr[3]];
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x == 3 && y == 3 {
                        (u32::from(p[6]) + 3 * u32::from(p[7]) + 2) >> 2
                    } else {
                        (u32::from(p[x + y])
                            + 2 * u32::from(p[x + y + 1])
                            + u32::from(p[x + y + 2])
                            + 2)
                            >> 2
                    };
                    out[y * 4 + x] = v as u8;
                }
            }
        }
        // Diagonal_Down_Right (spec 8.3.1.2.5), cell-for-cell like
        // FFmpeg's pred4x4_down_right.
        4 => {
            if !(nb.has_left && nb.has_top && tl.is_some()) {
                return Err(Error::BadValue("intra4x4 diag_dr missing samples"));
            }
            let c = tl.unwrap();
            let (l0, l1, l2, l3) = (l[0], l[1], l[2], l[3]);
            let (t0, t1, t2, t3) = (t[0], t[1], t[2], t[3]);
            let mut set = |x: usize, y: usize, v: u32| out[y * 4 + x] = v as u8;
            let a = |p: u8, q: u8, r: u8| {
                (u32::from(p) + 2 * u32::from(q) + u32::from(r) + 2) >> 2
            };
            set(0, 3, a(l3, l2, l1));
            set(0, 2, a(l2, l1, l0));
            set(1, 3, a(l2, l1, l0));
            set(0, 1, a(l1, l0, c));
            set(1, 2, a(l1, l0, c));
            set(2, 3, a(l1, l0, c));
            set(0, 0, a(l0, c, t0));
            set(1, 1, a(l0, c, t0));
            set(2, 2, a(l0, c, t0));
            set(3, 3, a(l0, c, t0));
            set(1, 0, a(c, t0, t1));
            set(2, 1, a(c, t0, t1));
            set(3, 2, a(c, t0, t1));
            set(2, 0, a(t0, t1, t2));
            set(3, 1, a(t0, t1, t2));
            set(3, 0, a(t1, t2, t3));
        }
        // Vertical_Right (spec 8.3.1.2.6); formulas transcribed
        // cell-for-cell from the spec table, same order as FFmpeg's
        // pred4x4_vertical_right.
        5 => {
            if !(nb.has_left && nb.has_top && tl.is_some()) {
                return Err(Error::BadValue("intra4x4 vert_right missing samples"));
            }
            let c = tl.unwrap();
            let (l0, l1, l2) = (l[0], l[1], l[2]);
            let (t0, t1, t2, t3) = (t[0], t[1], t[2], t[3]);
            let mut set = |x: usize, y: usize, v: u32| out[y * 4 + x] = v as u8;
            let a = |_x: usize, _y: usize, p: u8, q: u8, r: u8| {
                ((u32::from(p) + 2 * u32::from(q) + u32::from(r) + 2) >> 2) as u32
            };
            set(0, 0, u32::from(c) + u32::from(t0) + 1 >> 1);
            set(1, 2, (u32::from(c) + u32::from(t0) + 1) >> 1);
            set(1, 0, (u32::from(t0) + u32::from(t1) + 1) >> 1);
            set(2, 2, (u32::from(t0) + u32::from(t1) + 1) >> 1);
            set(2, 0, (u32::from(t1) + u32::from(t2) + 1) >> 1);
            set(3, 2, (u32::from(t1) + u32::from(t2) + 1) >> 1);
            set(3, 0, (u32::from(t2) + u32::from(t3) + 1) >> 1);
            set(0, 1, a(0, 1, l0, c, t0));
            set(1, 3, a(1, 3, l0, c, t0));
            set(1, 1, a(1, 1, c, t0, t1));
            set(2, 3, a(2, 3, c, t0, t1));
            set(2, 1, a(2, 1, t0, t1, t2));
            set(3, 3, a(3, 3, t0, t1, t2));
            set(3, 1, a(3, 1, t1, t2, t3));
            set(0, 2, a(0, 2, c, l0, l1));
            set(0, 3, a(0, 3, l0, l1, l2));
        }
        // Horizontal_Down (spec 8.3.1.2.8), cell-for-cell like FFmpeg's
        // pred4x4_horizontal_down.
        6 => {
            if !(nb.has_left && nb.has_top && tl.is_some()) {
                return Err(Error::BadValue("intra4x4 horiz_down missing samples"));
            }
            let c = tl.unwrap();
            let (l0, l1, l2, l3) = (l[0], l[1], l[2], l[3]);
            let (t0, t1, t2) = (t[0], t[1], t[2]);
            let mut set = |x: usize, y: usize, v: u32| out[y * 4 + x] = v as u8;
            let a = |p: u8, q: u8, r: u8| {
                (u32::from(p) + 2 * u32::from(q) + u32::from(r) + 2) >> 2
            };
            set(0, 0, (u32::from(c) + u32::from(l0) + 1) >> 1);
            set(2, 1, (u32::from(c) + u32::from(l0) + 1) >> 1);
            set(1, 0, a(l0, c, t0));
            set(3, 1, a(l0, c, t0));
            set(2, 0, a(c, t0, t1));
            set(3, 0, a(t0, t1, t2));
            set(0, 1, (u32::from(l0) + u32::from(l1) + 1) >> 1);
            set(2, 2, (u32::from(l0) + u32::from(l1) + 1) >> 1);
            set(1, 1, a(c, l0, l1));
            set(3, 2, a(c, l0, l1));
            set(0, 2, (u32::from(l1) + u32::from(l2) + 1) >> 1);
            set(2, 3, (u32::from(l1) + u32::from(l2) + 1) >> 1);
            set(1, 2, a(l0, l1, l2));
            set(3, 3, a(l0, l1, l2));
            set(0, 3, (u32::from(l2) + u32::from(l3) + 1) >> 1);
            set(1, 3, a(l1, l2, l3));
        }
        // Vertical_Left (spec 8.3.1.2.7), cell-for-cell like FFmpeg's
        // pred4x4_vertical_left.
        7 => {
            if !nb.has_top {
                return Err(Error::BadValue("intra4x4 vert_left without top"));
            }
            let tr = match &nb.top_right {
                Some(tr) => *tr,
                None => {
                    let mut a = [0u8; 16];
                    a[..4].fill(t[3]);
                    a
                }
            };
            let p: [u8; 8] = [t[0], t[1], t[2], t[3], tr[0], tr[1], tr[2], tr[3]];
            let mut set = |x: usize, y: usize, v: u32| out[y * 4 + x] = v as u8;
            let a = |i: usize| (u32::from(p[i]) + 2 * u32::from(p[i + 1]) + u32::from(p[i + 2]) + 2) >> 2;
            set(0, 0, (u32::from(p[0]) + u32::from(p[1]) + 1) >> 1);
            set(1, 0, (u32::from(p[1]) + u32::from(p[2]) + 1) >> 1);
            set(0, 2, (u32::from(p[1]) + u32::from(p[2]) + 1) >> 1);
            set(2, 0, (u32::from(p[2]) + u32::from(p[3]) + 1) >> 1);
            set(1, 2, (u32::from(p[2]) + u32::from(p[3]) + 1) >> 1);
            set(3, 0, (u32::from(p[3]) + u32::from(p[4]) + 1) >> 1);
            set(2, 2, (u32::from(p[3]) + u32::from(p[4]) + 1) >> 1);
            set(3, 2, (u32::from(p[4]) + u32::from(p[5]) + 1) >> 1);
            set(0, 1, a(0));
            set(1, 1, a(1));
            set(0, 3, a(1));
            set(2, 1, a(2));
            set(1, 3, a(2));
            set(3, 1, a(3));
            set(2, 3, a(3));
            set(3, 3, a(4));
        }
        // Horizontal_Up: left only.
        8 => {
            if !nb.has_left {
                return Err(Error::BadValue("intra4x4 horiz_up without left"));
            }
            for y in 0..4usize {
                for x in 0..4usize {
                    let zhu = x + 2 * y;
                    let v = if zhu == 0 || zhu == 2 || zhu == 4 {
                        // (p[-1, y + (x>>1)] + p[-1, y+(x>>1)+1] + 1)>>1
                        let yr = y + (x >> 1);
                        (u32::from(l[yr]) + u32::from(l[yr + 1]) + 1) >> 1
                    } else if zhu == 1 || zhu == 3 {
                        let yr = y + (x >> 1);
                        (u32::from(l[yr]) + 2 * u32::from(l[yr + 1]) + u32::from(l[yr + 2]) + 2) >> 2
                    } else if zhu == 5 {
                        (u32::from(l[2]) + 3 * u32::from(l[3]) + 2) >> 2
                    } else {
                        u32::from(l[3])
                    };
                    out[y * 4 + x] = v as u8;
                }
            }
        }
        _ => return Err(Error::BadValue("intra4x4 mode over 8")),
    }
    Ok(())
}

/// Intra-16x16 prediction (spec 8.3.3): modes 0..=3 = DC, H, V, Plane.
/// `top`/`left` must have 16 samples + `top_left`; availability of a
/// required side on a non-DC mode is a stream error.
pub(crate) fn pred16x16(mode: u8, nb: &NbSamples, out: &mut [u8; 256]) -> Result<()> {
    match mode {
        0 => {
            let v = match (nb.has_left, nb.has_top) {
                (true, true) => {
                    (nb.top[..16].iter().map(|&s| u32::from(s)).sum::<u32>()
                        + nb.left[..16].iter().map(|&s| u32::from(s)).sum::<u32>()
                        + 16)
                        >> 5
                }
                (true, false) => {
                    (nb.left[..16].iter().map(|&s| u32::from(s)).sum::<u32>() + 8) >> 4
                }
                (false, true) => {
                    (nb.top[..16].iter().map(|&s| u32::from(s)).sum::<u32>() + 8) >> 4
                }
                (false, false) => 128,
            } as u8;
            out.fill(v);
        }
        1 => {
            if !nb.has_left {
                return Err(Error::BadValue("intra16x16 horizontal without left"));
            }
            for y in 0..16 {
                out[y * 16..y * 16 + 16].fill(nb.left[y]);
            }
        }
        2 => {
            if !nb.has_top {
                return Err(Error::BadValue("intra16x16 vertical without top"));
            }
            for y in 0..16 {
                out[y * 16..y * 16 + 16].copy_from_slice(&nb.top[..16]);
            }
        }
        3 => {
            if !(nb.has_left && nb.has_top && nb.top_left.is_some()) {
                return Err(Error::BadValue("intra16x16 plane missing samples"));
            }
            let mut h = 0i32;
            let mut v = 0i32;
            for x in 1..=8usize {
                // p[7+x,-1] - p[7-x,-1]; p[-1,-1] for x=8 gives p[-1,-1].
                let hi = if x == 8 {
                    i32::from(nb.top[15])
                } else {
                    i32::from(nb.top[7 + x])
                };
                let lo = if x == 8 {
                    i32::from(nb.top_left.unwrap())
                } else {
                    i32::from(nb.top[7 - x])
                };
                h += x as i32 * (hi - lo);
                let hv = if x == 8 {
                    i32::from(nb.left[15])
                } else {
                    i32::from(nb.left[7 + x])
                };
                let lv = if x == 8 {
                    i32::from(nb.top_left.unwrap())
                } else {
                    i32::from(nb.left[7 - x])
                };
                v += x as i32 * (hv - lv);
            }
            let a = 16 * (i32::from(nb.left[15]) + i32::from(nb.top[15]));
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for y in 0..16 {
                for x in 0..16 {
                    out[y * 16 + x] = clip(a + b * (x as i32 - 7) + c * (y as i32 - 7) + 16 >> 5);
                }
            }
        }
        _ => return Err(Error::BadValue("intra16x16 mode over 3")),
    }
    Ok(())
}

/// 8x8 chroma prediction (spec 8.3.4): mode 0 DC (per-quadrant means
/// with per-side fallback), 1 horizontal, 2 vertical, 3 plane.
pub(crate) fn pred_chroma(mode: u8, nb: &NbSamples, out: &mut [u8; 64]) -> Result<()> {
    match mode {
        0 => {
            // Four 4x4 quadrants, each with its own DC mean. Spec
            // 8.3.4.2: quadrant (qx, qy) uses top samples qx*4..qx*4+4
            // and left samples qy*4..qy*4+4; a missing side doubles the
            // other side's sum; both missing gives 128.
            for qy in 0..2 {
                for qx in 0..2 {
                    let tr = &nb.top[qx * 4..qx * 4 + 4];
                    let lr = &nb.left[qy * 4..qy * 4 + 4];
                    // For quadrant 1 (x>=4, y<4) the spec substitutes the
                    // top-left half when p[4..8,-1] is unavailable — the
                    // caller encodes that by clearing the top-right range.
                    let ts: u32 = tr.iter().map(|&s| u32::from(s)).sum();
                    let ls: u32 = lr.iter().map(|&s| u32::from(s)).sum();
                    let v = match (nb.has_left, nb.has_top) {
                        (true, true) => ((ts + ls + 4) >> 3) as u8,
                        (true, false) => ((2 * ls + 4) >> 3) as u8,
                        (false, true) => ((2 * ts + 4) >> 3) as u8,
                        (false, false) => 128,
                    };
                    for y in 0..4 {
                        for x in 0..4 {
                            out[(qy * 4 + y) * 8 + qx * 4 + x] = v;
                        }
                    }
                }
            }
        }
        1 => {
            if !nb.has_left {
                return Err(Error::BadValue("chroma horizontal without left"));
            }
            for y in 0..8 {
                for x in 0..8 {
                    out[y * 8 + x] = nb.left[y];
                }
            }
        }
        2 => {
            if !nb.has_top {
                return Err(Error::BadValue("chroma vertical without top"));
            }
            for y in 0..8 {
                out[y * 8..y * 8 + 8].copy_from_slice(&nb.top[..8]);
            }
        }
        3 => {
            if !(nb.has_left && nb.has_top && nb.top_left.is_some()) {
                return Err(Error::BadValue("chroma plane missing samples"));
            }
            let mut h = 0i32;
            let mut v = 0i32;
            for x in 1..=4usize {
                let hi = if x == 4 {
                    i32::from(nb.top[7])
                } else {
                    i32::from(nb.top[3 + x])
                };
                let lo = if x == 4 {
                    i32::from(nb.top_left.unwrap())
                } else {
                    i32::from(nb.top[3 - x])
                };
                h += x as i32 * (hi - lo);
                let hv = if x == 4 {
                    i32::from(nb.left[7])
                } else {
                    i32::from(nb.left[3 + x])
                };
                let lv = if x == 4 {
                    i32::from(nb.top_left.unwrap())
                } else {
                    i32::from(nb.left[3 - x])
                };
                v += x as i32 * (hv - lv);
            }
            let a = 16 * (i32::from(nb.left[7]) + i32::from(nb.top[7]));
            let b = (17 * h + 16) >> 5;
            let c = (17 * v + 16) >> 5;
            for y in 0..8 {
                for x in 0..8 {
                    out[y * 8 + x] = clip(a + b * (x as i32 - 3) + c * (y as i32 - 3) + 16 >> 5);
                }
            }
        }
        _ => return Err(Error::BadValue("chroma mode over 3")),
    }
    Ok(())
}
