//! Chroma upsampling — transcription of libjpeg's "fancy" (triangle)
//! filters from jdsample.c so output matches the reference decoder
//! bit-for-bit: `h2v1_fancy_upsample`, `h2v2_fancy_upsample`,
//! `h1v2_fancy_upsample`, with fullsize passthrough and plain box
//! filters as the documented fallbacks for the degenerate geometry
//! (`downsampled_width <= 2`, matching libjpeg's own selection rule)
//! and for sampling factors above 2 (generic `int_upsample`
//! replication, which libjpeg itself falls back to for e.g. 4:1:1).
//!
//! All filters operate on a downsampled plane of `down_w × down_h`
//! samples (the *true* sample grid, not the MCU-padded block grid — the
//! padded columns/rows of the source blocks are never referenced) and
//! produce an `out_w × out_h` plane.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::parser::{Component, Frame};

/// Upsamples one component plane to frame resolution.
pub(crate) fn to_full_size(
    comp: &Component,
    plane: &[u8],
    stride: usize,
    frame: &Frame,
) -> Result<Vec<u8>> {
    let out_w = frame.width;
    let out_h = frame.height;
    let (dw, dh) = (comp.down_w, comp.down_h);

    // Sample counts this component contributes per output group.
    let h_in = comp.h;
    let v_in = comp.v;
    let h_out = frame.hmax;
    let v_out = frame.vmax;

    if h_in == h_out && v_in == v_out {
        // Fullsize: crop the padded plane to the frame.
        return Ok(crop(plane, stride, dw, out_w, out_h));
    }
    if h_in * 2 == h_out && v_in == v_out {
        if dw > 2 {
            return Ok(h2v1_fancy(plane, stride, dw, out_w, out_h));
        }
        return Ok(box_x2(plane, stride, dw, dh, out_w, out_h));
    }
    if h_in == h_out && v_in * 2 == v_out {
        return Ok(h1v2_fancy(plane, stride, dh, out_w, out_h));
    }
    if h_in * 2 == h_out && v_in * 2 == v_out {
        if dw > 2 {
            return Ok(h2v2_fancy(plane, stride, dw, dh, out_w, out_h));
        }
        return Ok(box_x2y2(plane, stride, dw, dh, out_w, out_h));
    }
    if h_out % h_in == 0 && v_out % v_in == 0 {
        // Generic integral replication (libjpeg int_upsample) — covers
        // 3:1, 4:1 etc.
        return Ok(replicate(
            plane,
            stride,
            (dw, dh),
            (out_w, out_h),
            (h_out / h_in, v_out / v_in),
        ));
    }
    Err(Error::Unsupported(
        "fractional sampling ratio (non-integral upsample)",
    ))
}

/// Extracts the top-left `w × h` region of a `stride`-pitched plane.
fn crop(plane: &[u8], stride: usize, _w_in: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = alloc::vec![0u8; w * h];
    for y in 0..h {
        out[y * w..y * w + w].copy_from_slice(&plane[y * stride..y * stride + w]);
    }
    out
}

#[inline]
fn px(plane: &[u8], stride: usize, x: usize, y: usize) -> i32 {
    i32::from(plane[y * stride + x])
}

/// h2v1 triangle filter (jdsample.c h2v1_fancy_upsample).
///
/// The filter emits `2*dw` samples per row; when the frame width is odd
/// that is `out_w + 1`, so rows are staged at pitch `2*dw` and cropped —
/// libjpeg does the same (its output rows are padded, the color stage
/// reads only `output_width`).
fn h2v1_fancy(plane: &[u8], stride: usize, dw: usize, out_w: usize, out_h: usize) -> Vec<u8> {
    let pitch = 2 * dw;
    let mut wide = alloc::vec![0u8; pitch * out_h];
    for r in 0..out_h {
        let mut o = r * pitch;
        // First column: out[0] = in[0]; out[1] = (3*in[0] + in[1] + 2) >> 2
        let first = px(plane, stride, 0, r);
        wide[o] = first as u8;
        wide[o + 1] = ((first * 3 + px(plane, stride, 1, r) + 2) >> 2) as u8;
        o += 2;
        for c in 1..dw - 1 {
            let v = px(plane, stride, c, r) * 3;
            wide[o] = ((v + px(plane, stride, c - 1, r) + 1) >> 2) as u8;
            wide[o + 1] = ((v + px(plane, stride, c + 1, r) + 2) >> 2) as u8;
            o += 2;
        }
        let last = px(plane, stride, dw - 1, r);
        wide[o] = ((last * 3 + px(plane, stride, dw - 2, r) + 1) >> 2) as u8;
        wide[o + 1] = last as u8;
    }
    crop(&wide, pitch, 0, out_w, out_h)
}

/// h1v2 triangle filter (jdsample.c h1v2_fancy_upsample): 3:1 blend of
/// the nearer input row against the next nearest.
fn h1v2_fancy(plane: &[u8], stride: usize, dh: usize, out_w: usize, out_h: usize) -> Vec<u8> {
    let mut out = alloc::vec![0u8; out_w * out_h];
    let mut inrow = 0usize;
    let mut outrow = 0usize;
    while outrow < out_h {
        for v in 0..2 {
            if outrow >= out_h {
                break;
            }
            let (near, far, bias) = if v == 0 {
                (inrow, inrow.saturating_sub(1), 1i32)
            } else {
                (inrow, (inrow + 1).min(dh.saturating_sub(1)), 2i32)
            };
            for x in 0..out_w {
                let colsum = px(plane, stride, x, near) * 3 + px(plane, stride, x, far);
                out[outrow * out_w + x] = ((colsum + bias) >> 2) as u8;
            }
            outrow += 1;
        }
        inrow += 1;
    }
    out
}

/// h2v2 triangle filter (jdsample.c h2v2_fancy_upsample): 9:3:3:1 blend.
/// Same `2*dw` pitch staging as [`h2v1_fancy`] for odd frame widths.
fn h2v2_fancy(
    plane: &[u8],
    stride: usize,
    dw: usize,
    dh: usize,
    out_w: usize,
    out_h: usize,
) -> Vec<u8> {
    let pitch = 2 * dw;
    let mut wide = alloc::vec![0u8; pitch * out_h];
    let mut inrow = 0usize;
    let mut outrow = 0usize;
    while outrow < out_h {
        for v in 0..2 {
            if outrow >= out_h {
                break;
            }
            let near = inrow;
            let far = if v == 0 {
                inrow.saturating_sub(1)
            } else {
                (inrow + 1).min(dh - 1)
            };
            let row = outrow * pitch;
            // colsum for input column c: 3*near[c] + far[c]
            let colsum =
                |c: usize| -> i32 { px(plane, stride, c, near) * 3 + px(plane, stride, c, far) };
            let mut thiscol = colsum(0);
            let mut next = colsum(1);
            wide[row] = ((thiscol * 4 + 8) >> 4) as u8;
            wide[row + 1] = ((thiscol * 3 + next + 7) >> 4) as u8;
            let mut last = thiscol;
            thiscol = next;
            let mut o = row + 2;
            for c in 1..dw - 1 {
                next = colsum(c + 1);
                wide[o] = ((thiscol * 3 + last + 8) >> 4) as u8;
                wide[o + 1] = ((thiscol * 3 + next + 7) >> 4) as u8;
                last = thiscol;
                thiscol = next;
                o += 2;
            }
            wide[o] = ((thiscol * 3 + last + 8) >> 4) as u8;
            wide[o + 1] = ((thiscol * 4 + 7) >> 4) as u8;
            outrow += 1;
        }
        inrow += 1;
    }
    crop(&wide, pitch, 0, out_w, out_h)
}

/// Plain 2× horizontal box (libjpeg h2v1_upsample fallback).
fn box_x2(
    plane: &[u8],
    stride: usize,
    dw: usize,
    _dh: usize,
    out_w: usize,
    out_h: usize,
) -> Vec<u8> {
    let mut out = alloc::vec![0u8; out_w * out_h];
    for y in 0..out_h {
        for x in 0..out_w {
            out[y * out_w + x] = plane[y * stride + (x / 2).min(dw - 1)];
        }
    }
    out
}

/// Plain 2×2 box (libjpeg h2v2_upsample fallback).
fn box_x2y2(
    plane: &[u8],
    stride: usize,
    dw: usize,
    dh: usize,
    out_w: usize,
    out_h: usize,
) -> Vec<u8> {
    let mut out = alloc::vec![0u8; out_w * out_h];
    for y in 0..out_h {
        for x in 0..out_w {
            out[y * out_w + x] = plane[(y / 2).min(dh - 1) * stride + (x / 2).min(dw - 1)];
        }
    }
    out
}

/// Generic integral replication (libjpeg int_upsample).
fn replicate(
    plane: &[u8],
    stride: usize,
    dims: (usize, usize),
    out: (usize, usize),
    expand: (usize, usize),
) -> Vec<u8> {
    let (dw, dh) = dims;
    let (out_w, out_h) = out;
    let (hx, vx) = expand;
    let mut out = alloc::vec![0u8; out_w * out_h];
    for y in 0..out_h {
        let sy = (y / vx).min(dh - 1);
        for x in 0..out_w {
            out[y * out_w + x] = plane[sy * stride + (x / hx).min(dw - 1)];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known-answer h2v1 triangle: input [10, 20, 30, 40] must produce
    /// libjpeg's exact 8-sample row (verified against jdsample.c).
    #[test]
    fn h2v1_known_answer() {
        let plane = [10u8, 20, 30, 40];
        let out = h2v1_fancy(&plane, 4, 4, 8, 1);
        // out[0]=10; out[1]=(30+20+2)/4=13; pairs for c=1: (60+10+1)/4=17,
        // (60+30+2)/4=23; c=2: (90+20+1)/4=27, (90+40+2)/4=33;
        // last: (120+30+1)/4=37, 40.
        assert_eq!(out, vec![10, 13, 17, 23, 27, 33, 37, 40]);
    }

    /// h2v2 on a 2×2 plane: every output pixel is a 9:3:3:1 blend with
    /// edge replication; verify the exact integer result.
    #[test]
    fn h2v2_known_answer() {
        // plane: [0, 100; 200, 255] → 4×4 out
        let plane = [0u8, 100, 200, 255];
        let out = h2v2_fancy(&plane, 2, 2, 2, 4, 4);
        // Row 0 (v=0): near=row0, far=row0 (clamped). colsums: c0=0,
        // c1=400.
        // out[0] = (0*4+8)>>4 = 0; out[1] = (0*3+400+7)>>4 = 25.
        // Last col: (400*3+0+8)>>4 = 75; (400*4+7)>>4 = 100.
        assert_eq!(&out[0..4], &[0, 25, 75, 100]);
        // Row 1 (v=1): near=row0, far=row1. colsums: c0 = 0*3+200 = 200;
        // c1 = 100*3+255 = 555.
        // out[4] = (200*4+8)>>4 = 50; out[5] = (200*3+555+7)>>4 = 72;
        // last: (555*3+200+8)>>4 = 117; (555*4+7)>>4 = 139.
        assert_eq!(&out[4..8], &[50, 72, 117, 139]);
    }

    /// Integral replication for exotic factors (4:1 etc.).
    #[test]
    fn replicate_4x() {
        let plane = [9u8, 8, 7, 6];
        let out = replicate(&plane, 4, (4, 1), (8, 2), (4, 2));
        assert_eq!(&out[0..8], &[9, 9, 9, 9, 8, 8, 8, 8]);
        assert_eq!(&out[8..16], &[9, 9, 9, 9, 8, 8, 8, 8]);
    }
}
