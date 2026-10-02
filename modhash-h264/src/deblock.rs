//! In-loop deblocking filter (spec 8.7), frame-picture subset:
//! vertical luma edges left-to-right per 4-sample set, then horizontal,
//! then the chroma edges. Boundary strength per spec 8.7.2.1 with
//! `mixedModeEdgeFlag = 0` (frame pictures only), threshold derivation
//! per 8.7.2.2 (Tables 8-16/8-17), weak filtering per 8.7.2.3 and the
//! strong intra path per 8.7.2.4.
//!
//! The two tuneable knobs come from each MB's own slice (spec: the
//! values of the slice containing sample q0): `disable_deblocking_
//! filter_idc` 1 disables filtering for that MB's controlled edges,
//! and idc 2 additionally skips edges that sit on slice boundaries.

use crate::mb::{MbState, MbType};
use crate::tables::{block_index, ALPHA_TABLE, BETA_TABLE, TC0_TABLE};
use alloc::vec::Vec;

/// Boundary strengths for one MB: vertical edges at luma x = {0,4,8,12}
/// and horizontal at y = {0,4,8,12}; each entry is the strength of the
/// 4-sample set `k`.
#[derive(Clone, Copy)]
struct BsMap {
    v: [[u8; 4]; 4],
    h: [[u8; 4]; 4],
}

fn is_intra(t: MbType) -> bool {
    t.is_intra()
}



/// Per-MB boundary-strength computation (spec 8.7.2.1, frame subset).
/// `mb_a`/`mb_b` are the left/top neighbours of `cur`.
fn compute_bs(
    cur: &MbState,
    mb_a: Option<&MbState>,
    mb_b: Option<&MbState>,
) -> BsMap {
    let mut m = BsMap {
        v: [[0; 4]; 4],
        h: [[0; 4]; 4],
    };
    let cur_intra = is_intra(cur.mb_type);

    for e in 0..4 {
        // Edge is an MB edge when e == 0.
        for k in 0..4usize {
            // ---- vertical edge at x4 = e, block row k ----
            let bsv = if e == 0 {
                let nb = match mb_a {
                    Some(m) => m,
                    None => continue, // picture boundary: not filtered
                };
                if nb.slice_id != cur.slice_id && cur.disable_deblock_idc == 2 {
                    continue;
                }
                let p_blk = block_index(3, k);
                let q_blk = block_index(0, k);
                if cur_intra || is_intra(nb.mb_type) {
                    4
                } else if cur.nz[q_blk] > 0 || nb.nz[p_blk] > 0 {
                    2
                } else {
                    mv_diff_bs(cur.mv[q_blk], nb.mv[p_blk], cur.ref_idx[q_blk], nb.ref_idx[p_blk])
                }
            } else {
                let p_blk = block_index(e - 1, k);
                let q_blk = block_index(e, k);
                if cur_intra {
                    3
                } else if cur.nz[p_blk] > 0 || cur.nz[q_blk] > 0 {
                    2
                } else {
                    mv_diff_bs(
                        cur.mv[q_blk],
                        cur.mv[p_blk],
                        cur.ref_idx[q_blk],
                        cur.ref_idx[p_blk],
                    )
                }
            };
            m.v[e][k] = bsv;

            // ---- horizontal edge at y4 = e, block column k ----
            let bsh = if e == 0 {
                let nb = match mb_b {
                    Some(m) => m,
                    None => continue,
                };
                if nb.slice_id != cur.slice_id && cur.disable_deblock_idc == 2 {
                    continue;
                }
                let p_blk = block_index(k, 3);
                let q_blk = block_index(k, 0);
                if cur_intra || is_intra(nb.mb_type) {
                    4
                } else if cur.nz[q_blk] > 0 || nb.nz[p_blk] > 0 {
                    2
                } else {
                    mv_diff_bs(cur.mv[q_blk], nb.mv[p_blk], cur.ref_idx[q_blk], nb.ref_idx[p_blk])
                }
            } else {
                let p_blk = block_index(k, e - 1);
                let q_blk = block_index(k, e);
                if cur_intra {
                    3
                } else if cur.nz[p_blk] > 0 || cur.nz[q_blk] > 0 {
                    2
                } else {
                    mv_diff_bs(
                        cur.mv[q_blk],
                        cur.mv[p_blk],
                        cur.ref_idx[q_blk],
                        cur.ref_idx[p_blk],
                    )
                }
            };
            m.h[e][k] = bsh;
        }
    }
    m
}

/// bS = 1 when the reference pictures differ or a motion vector differs
/// by >= 4 quarter-pel (spec 8.7.2.1 last rules, frame subset).
fn mv_diff_bs(mv_q: [i16; 2], mv_p: [i16; 2], ref_q: u8, ref_p: u8) -> u8 {
    if ref_q != ref_p {
        return 1;
    }
    if ref_q == 0xff {
        // Neither side inter-coded (skip blocks with no residual keep
        // the picture's default vector — same picture, same vector).
        return 0;
    }
    if (i32::from(mv_q[0]) - i32::from(mv_p[0])).abs() >= 4
        || (i32::from(mv_q[1]) - i32::from(mv_p[1])).abs() >= 4
    {
        1
    } else {
        0
    }
}

fn clip3(lo: i32, hi: i32, v: i32) -> i32 {
    v.clamp(lo, hi)
}

/// One set of 4 samples across an edge, luma (`chroma = false`) or
/// chroma. `p` = samples p3..p0 (p0 adjacent), `q` = q0..q3. Returns
/// the filtered p0..p2 / q0..q2 as `(p', q')` with 3 valid entries.
fn filter_set(
    p: &[i32; 4],
    q: &[i32; 4],
    bs: u8,
    qp_av: i32,
    off_a: i32,
    off_b: i32,
    chroma: bool,
) -> ([i32; 3], [i32; 3]) {
    let index_a = clip3(0, 51, qp_av + off_a) as usize;
    let index_b = clip3(0, 51, qp_av + off_b) as usize;
    let alpha = i32::from(ALPHA_TABLE[index_a]);
    let beta = i32::from(BETA_TABLE[index_b]);
    // Callers pass p = [p0, p1, p2, p3], q = [q0..q3].
    let (p0, p1, p2, p3) = (p[0], p[1], p[2], p[3]);
    let (q0, q1, q2, q3) = (q[0], q[1], q[2], q[3]);
    let mut po = [p0, p1, p2];
    let mut qo = [q0, q1, q2];
    if bs == 0 || (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return (po, qo);
    }
    let ap = (p2 - p0).abs();
    let aq = (q2 - q0).abs();
    if bs < 4 {
        let tc0 = i32::from(TC0_TABLE[index_a][(bs - 1) as usize]);
        let tc = if chroma {
            tc0 + 1
        } else {
            tc0 + i32::from(ap < beta) + i32::from(aq < beta)
        };
        let delta = clip3(-tc, tc, ((q0 - p0) * 4 + (p1 - q1) + 4) >> 3);
        po[0] = (p0 + delta).clamp(0, 255);
        qo[0] = (q0 - delta).clamp(0, 255);
        if !chroma {
            if ap < beta {
                po[1] = p1 + clip3(-tc0, tc0, (p2 + ((p0 + q0 + 1) >> 1) - 2 * p1) >> 1);
            }
            if aq < beta {
                qo[1] = q1 + clip3(-tc0, tc0, (q2 + ((p0 + q0 + 1) >> 1) - 2 * q1) >> 1);
            }
        }
    } else {
        // bS == 4 (spec 8.7.2.4); chroma edges always take the weak
        // branch of 8-480/8-487 (chromaStyleFilteringFlag = 1).
        if !chroma && ap < beta && (p0 - q0).abs() < (alpha >> 2) + 2 {
            po[0] = (p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3;
            po[1] = (p2 + p1 + p0 + q0 + 2) >> 2;
            po[2] = (2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3;
        } else {
            po[0] = (2 * p1 + p0 + q1 + 2) >> 2;
        }
        if !chroma && aq < beta && (p0 - q0).abs() < (alpha >> 2) + 2 {
            qo[0] = (p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3;
            qo[1] = (p0 + q0 + q1 + q2 + 2) >> 2;
            qo[2] = (2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3;
        } else {
            qo[0] = (2 * q1 + q0 + p1 + 2) >> 2;
        }
    }
    (po, qo)
}

/// Filters a vertical edge: 4 sets of samples, each on row `y0+k`,
/// crossing column `x`. `plane` row-major, `stride` bytes.
fn filter_v_edge(
    plane: &mut [u8],
    stride: usize,
    x: usize,
    y0: usize,
    bs: &[u8; 4],
    qp_p: &[u8; 4],
    qp_q: u8,
    off_a: i32,
    off_b: i32,
    chroma: bool,
) {
    for (k, &b) in bs.iter().enumerate() {
        if b == 0 {
            continue;
        }
        let y = y0 + k;
        let row = &mut plane[y * stride..y * stride + stride];
        let p: [i32; 4] = [
            i32::from(row[x - 1]),
            i32::from(row[x - 2]),
            i32::from(row[x - 3]),
            i32::from(row[x - 4]),
        ];
        let q: [i32; 4] = [
            i32::from(row[x]),
            i32::from(row[x + 1]),
            i32::from(row[x + 2]),
            i32::from(row[x + 3]),
        ];
        // Order for filter_set: [p0..p3] ascending distance.
        let p_ord = [p[0], p[1], p[2], p[3]];
        let qp_av = (i32::from(qp_p[k]) + i32::from(qp_q) + 1) >> 1;
        let (po, qo) = filter_set(&p_ord, &q, b, qp_av, off_a, off_b, chroma);
        row[x - 1] = po[0].clamp(0, 255) as u8;
        row[x - 2] = po[1].clamp(0, 255) as u8;
        row[x - 3] = po[2].clamp(0, 255) as u8;
        row[x] = qo[0].clamp(0, 255) as u8;
        row[x + 1] = qo[1].clamp(0, 255) as u8;
        row[x + 2] = qo[2].clamp(0, 255) as u8;
    }
}

/// Filters a horizontal edge: 4 sets of samples, each on column `x0+k`,
/// crossing row `y`.
fn filter_h_edge(
    plane: &mut [u8],
    stride: usize,
    x0: usize,
    y: usize,
    bs: &[u8; 4],
    qp_p: &[u8; 4],
    qp_q: u8,
    off_a: i32,
    off_b: i32,
    chroma: bool,
) {
    for (k, &b) in bs.iter().enumerate() {
        if b == 0 {
            continue;
        }
        let x = x0 + k;
        let px = |dy: i32| i32::from(plane[(y as i32 + dy) as usize * stride + x]);
        let p: [i32; 4] = [px(-1), px(-2), px(-3), px(-4)];
        let q: [i32; 4] = [px(0), px(1), px(2), px(3)];
        let qp_av = (i32::from(qp_p[k]) + i32::from(qp_q) + 1) >> 1;
        let (po, qo) = filter_set(&p, &q, b, qp_av, off_a, off_b, chroma);
        plane[(y - 1) * stride + x] = po[0].clamp(0, 255) as u8;
        plane[(y - 2) * stride + x] = po[1].clamp(0, 255) as u8;
        plane[(y - 3) * stride + x] = po[2].clamp(0, 255) as u8;
        plane[y * stride + x] = qo[0].clamp(0, 255) as u8;
        plane[(y + 1) * stride + x] = qo[1].clamp(0, 255) as u8;
        plane[(y + 2) * stride + x] = qo[2].clamp(0, 255) as u8;
    }
}

/// Runs the whole deblocking pass over a reconstructed frame.
///
/// `y`, `cb`, `cr` are the picture planes (row-major, `stride` luma
/// and `stride/2` chroma); `mbs` is the per-MB state array indexed by
/// `mb_y * width_mbs + mb_x`. `qp_chroma` maps a luma QP to the chroma
/// QP the spec uses on chroma edges (Table 8-15 via the PPS offset).
pub(crate) fn filter_frame(
    y: &mut [u8],
    cb: &mut [u8],
    cr: &mut [u8],
    stride: usize,
    mbs: &[MbState],
    width_mbs: usize,
    height_mbs: usize,
    qp_chroma: &[u8; 52],
) {
    // Per-MB boundary strengths first (they're read for chroma too and
    // are independent of filtering order).
    let mut maps: Vec<BsMap> = Vec::with_capacity(mbs.len());
    for mb_y in 0..height_mbs {
        for mb_x in 0..width_mbs {
            let idx = mb_y * width_mbs + mb_x;
            let cur = &mbs[idx];
            let a = if mb_x > 0 { Some(&mbs[idx - 1]) } else { None };
            let b = if mb_y > 0 {
                Some(&mbs[idx - width_mbs])
            } else {
                None
            };
            maps.push(compute_bs(cur, a, b));
        }
    }

    for mb_y in 0..height_mbs {
        for mb_x in 0..width_mbs {
            let idx = mb_y * width_mbs + mb_x;
            let cur = &mbs[idx];
            if cur.disable_deblock_idc == 1 {
                continue;
            }
            let bs = maps[idx];
            let off_a = i32::from(cur.filter_offset_a);
            let off_b = i32::from(cur.filter_offset_b);
            let off_ca = off_a; // chroma uses the same slice offsets
            let off_cb = off_b;
            let _ = (off_ca, off_cb);

            // LUMA vertical edges x = {0,4,8,12} of this MB.
            for e in 0..4usize {
                let x = mb_x * 16 + e * 4;
                let mut qp_p = [0u8; 4];
                if e == 0 {
                    if mb_x == 0 {
                        continue;
                    }
                    let nb = &mbs[idx - 1];
                    if nb.slice_id != cur.slice_id && cur.disable_deblock_idc == 2 {
                        continue;
                    }
                    for k in 0..4 {
                        qp_p[k] = nb.qp_y;
                    }
                } else {
                    for k in 0..4 {
                        qp_p[k] = cur.qp_y;
                    }
                }
                filter_v_edge(
                    y,
                    stride,
                    x,
                    mb_y * 16,
                    &bs.v[e],
                    &qp_p,
                    cur.qp_y,
                    off_a,
                    off_b,
                    false,
                );
            }

            // LUMA horizontal edges y = {0,4,8,12}.
            for e in 0..4usize {
                let yy = mb_y * 16 + e * 4;
                let mut qp_p = [0u8; 4];
                if e == 0 {
                    if mb_y == 0 {
                        continue;
                    }
                    let nb = &mbs[idx - width_mbs];
                    if nb.slice_id != cur.slice_id && cur.disable_deblock_idc == 2 {
                        continue;
                    }
                    for k in 0..4 {
                        qp_p[k] = nb.qp_y;
                    }
                } else {
                    for k in 0..4 {
                        qp_p[k] = cur.qp_y;
                    }
                }
                filter_h_edge(
                    y,
                    stride,
                    mb_x * 16,
                    yy,
                    &bs.h[e],
                    &qp_p,
                    cur.qp_y,
                    off_a,
                    off_b,
                    false,
                );
            }

            // CHROMA: vertical edges at chroma x = {0, 4} (luma {0, 8});
            // horizontal at chroma y = {0, 4}. Strength = luma bS of
            // the edge containing the luma sample (2x, 2y).
            for plane in [&mut *cb, &mut *cr] {
                for e in 0..2usize {
                    let lx = e * 2; // luma edge index 0 or 2
                    // bS for chroma k-set: luma bS at block-row k/2.
                    let mut bs4 = [0u8; 4];
                    for k in 0..4 {
                        bs4[k] = bs.v[lx][k / 2];
                    }
                    let x = mb_x * 8 + e * 4;
                    let mut qp_p = [0u8; 4];
                    if e == 0 {
                        if mb_x == 0 {
                            continue;
                        }
                        let nb = &mbs[idx - 1];
                        if nb.slice_id != cur.slice_id && cur.disable_deblock_idc == 2 {
                            continue;
                        }
                        for k in 0..4 {
                            qp_p[k] = qp_chroma[nb.qp_y as usize];
                        }
                    } else {
                        for k in 0..4 {
                            qp_p[k] = qp_chroma[cur.qp_y as usize];
                        }
                    }
                    // 8 chroma sample rows per edge = two 4-sets.
                    for half in 0..2usize {
                        filter_v_edge(
                            plane,
                            stride / 2,
                            x,
                            mb_y * 8 + half * 4,
                            &bs4,
                            &qp_p,
                            qp_chroma[cur.qp_y as usize],
                            off_a,
                            off_b,
                            true,
                        );
                    }
                }
                for e in 0..2usize {
                    let ly = e * 2;
                    let mut bs4 = [0u8; 4];
                    for k in 0..4 {
                        bs4[k] = bs.h[ly][k / 2];
                    }
                    let yy = mb_y * 8 + e * 4;
                    let mut qp_p = [0u8; 4];
                    if e == 0 {
                        if mb_y == 0 {
                            continue;
                        }
                        let nb = &mbs[idx - width_mbs];
                        if nb.slice_id != cur.slice_id && cur.disable_deblock_idc == 2 {
                            continue;
                        }
                        for k in 0..4 {
                            qp_p[k] = qp_chroma[nb.qp_y as usize];
                        }
                    } else {
                        for k in 0..4 {
                            qp_p[k] = qp_chroma[cur.qp_y as usize];
                        }
                    }
                    for half in 0..2usize {
                        filter_h_edge(
                            plane,
                            stride / 2,
                            mb_x * 8 + half * 4,
                            yy,
                            &bs4,
                            &qp_p,
                            qp_chroma[cur.qp_y as usize],
                            off_a,
                            off_b,
                            true,
                        );
                    }
                }
            }
        }
    }
}
