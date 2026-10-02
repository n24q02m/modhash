//! FAST-9 corner detection and steered-BRIEF (rBRIEF) descriptors — the
//! spec §4.3 ORB recipe in miniature: find corner pixels on a 16-pixel
//! circle, take a 31×31 patch, steer a fixed 256-pair test pattern by the
//! patch's intensity-centroid orientation, and emit a 256-bit descriptor.
//!
//! The pinned constants come from the design spec and the ORB paper
//! (Rublee et al., ICCV 2011):
//!
//! * [`FAST9_THRESHOLD`] = 20 — the circle-vs-centre contrast floor.
//! * [`FAST9_ARC`] = 9 — the minimum contiguous run of circle pixels
//!   beyond the threshold that makes a corner.
//! * [`PATCH_RADIUS`] = 15 — BRIEF's 31×31 patch, and the radius of the
//!   disk the 256 test pairs are generated inside. Because every pair
//!   offset satisfies `x² + y² ≤ 225`, rotating it by any angle keeps it
//!   inside the same disk: a 15-pixel border is sufficient for steered
//!   sampling (no patch padding beyond the nominal 31×31).
//! * [`BRIEF_SEED`] — the fixed splitmix64 seed the pair table is
//!   generated with. Any seed would do; this one is part of the format:
//!   descriptors are only comparable between runs of the same table.
//!
//! Orientations are measured in radians with
//! `θ = atan2(m01, m10)` over the full 31×31 patch (image moments,
//! OpenCV ORB convention), and the descriptor bit order is MSB-first
//! inside each byte, matching the kit's row-major/MSB-first bitpacking
//! convention.
//!
//! Hostile inputs cannot panic: descriptor requests that reach outside
//! the image return `None`, never index out of bounds.

use alloc::vec::Vec;

use modhash_raster::{Gray, Image};

use crate::prng::splitmix64_next;

/// FAST contrast threshold from spec §4.3 (grey levels).
pub const FAST9_THRESHOLD: i32 = 20;

/// Contiguous circle pixels that must all pass the threshold — the "9"
/// in FAST-9.
pub const FAST9_ARC: usize = 9;

/// Half-size of the BRIEF patch: 31×31, radius 15.
pub const PATCH_RADIUS: i32 = 15;

/// Fixed seed generating [`BRIEF_PAIRS`]. Pinned, never versioned:
/// changing it changes every descriptor the crate produces.
pub const BRIEF_SEED: u64 = 0x4F52_4233_3236_3130; // "ORB32610"-ish tag

/// Number of point-pair tests in the descriptor — 256 bits.
pub const BRIEF_BITS: usize = 256;

/// The sixteen ring offsets of the FAST circle, clockwise from the top
/// pixel `(0, −3)` — the canonical ordering, `(dx, dy)` per entry.
const CIRCLE: [(i32, i32); 16] = [
    (0, -3),
    (1, -3),
    (2, -2),
    (3, -1),
    (3, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 3),
    (-1, 3),
    (-2, 2),
    (-3, 1),
    (-3, 0),
    (-3, -1),
    (-2, -2),
    (-1, -3),
];

/// The 256 steered-BRIEF test pairs, generated at compile time.
///
/// Each entry is `((x1, y1), (x2, y2))` inside the radius-15 disk; the
/// bit for the pair is set when the first sampled pixel is darker than
/// the second. Generated deterministically from [`BRIEF_SEED`], uniform
/// on `−15..=15` per coordinate with points outside the disk resampled
/// — uncorrelated, fixed forever.
pub static BRIEF_PAIRS: [((i8, i8), (i8, i8)); BRIEF_BITS] = gen_pairs();

const fn gen_pairs() -> [((i8, i8), (i8, i8)); BRIEF_BITS] {
    let mut pairs = [((0i8, 0i8), (0i8, 0i8)); BRIEF_BITS];
    let mut state = BRIEF_SEED;
    let mut i = 0;
    while i < BRIEF_BITS {
        // Four coordinates per pair; each resampled until it lands in
        // the radius-15 disk so steering can never leave the patch.
        let mut coords = [0i32; 4];
        let mut c = 0;
        while c < 4 {
            let v = (splitmix64_next(&mut state) % 31) as i32 - 15;
            coords[c] = v;
            c += 1;
        }
        let (x1, y1, x2, y2) = (coords[0], coords[1], coords[2], coords[3]);
        if x1 * x1 + y1 * y1 > 225 || x2 * x2 + y2 * y2 > 225 {
            continue;
        }
        pairs[i] = ((x1 as i8, y1 as i8), (x2 as i8, y2 as i8));
        i += 1;
    }
    pairs
}

/// A detected corner: image coordinates plus the FAST score.
///
/// `score` is the maximum threshold at which `(x, y)` is still a
/// FAST-9 corner — the ORB/Harris-free ordering key for `max_corners`
/// selection and non-maximum suppression. A circle on which the weakest
/// arc pixel differs from the centre by exactly the running threshold
/// scores `threshold`; larger score means a stronger corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Corner {
    /// Column.
    pub x: u32,
    /// Row.
    pub y: u32,
    /// Maximum passing threshold (≥ the detection threshold).
    pub score: i32,
}

/// A 256-bit rBRIEF descriptor, MSB-first inside each byte.
///
/// `PartialEq` compares all 256 bits; [`hamming256`] gives the matching
/// distance the index consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brief256(pub [u8; 32]);

/// Hamming distance between two 256-bit descriptors.
pub fn hamming256(a: &Brief256, b: &Brief256) -> u32 {
    let mut d = 0;
    for i in 0..32 {
        d += (a.0[i] ^ b.0[i]).count_ones();
    }
    d
}

/// `(x, y, grey)` at a signed offset, or `None` outside the image.
#[inline]
fn at(img: &Image<Gray, u8>, x: i32, y: i32) -> Option<i32> {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return None;
    }
    Some(i32::from(
        img.as_slice()[(y as u32 * img.width() + x as u32) as usize],
    ))
}

/// FAST ring values around `(x, y)`: `None` if any of the 16 ring
/// pixels lies outside the image (a 3-pixel border is unscoreable).
fn ring(img: &Image<Gray, u8>, x: u32, y: u32) -> Option<[i32; 16]> {
    let (x, y) = (x as i32, y as i32);
    let mut v = [0i32; 16];
    for (i, &(dx, dy)) in CIRCLE.iter().enumerate() {
        v[i] = at(img, x + dx, y + dy)?;
    }
    Some(v)
}

/// The tightest same-sign difference inside the best 9-arc: the minimum
/// of `|v − p|` over the contiguous window whose elements all share a
/// sign. `0` when no arc exists. A centre is a corner at threshold `t`
/// iff `score > t` — the strict inequality is the spec's
/// "more than the threshold" comparison, and it is what makes a pixel
/// exactly `t` away not qualify.
fn best_score(ring: &[i32; 16], p: i32) -> i32 {
    let mut best = 0;
    for s in 0..16 {
        // A window only qualifies when every difference shares a sign.
        let mut all_bright = true;
        let mut all_dark = true;
        let mut min_d = i32::MAX;
        for k in 0..FAST9_ARC {
            let d = ring[(s + k) % 16] - p;
            if d <= 0 {
                all_bright = false;
            }
            if d >= 0 {
                all_dark = false;
            }
            if d.abs() < min_d {
                min_d = d.abs();
            }
        }
        if (all_bright || all_dark) && min_d > best {
            best = min_d;
        }
    }
    best
}

/// Every FAST-9 corner of `img` at [`FAST9_THRESHOLD`], in row-major
/// order. Corners need a 3-pixel margin; images smaller than 7×7 return
/// an empty vector.
pub fn fast9(img: &Image<Gray, u8>) -> Vec<Corner> {
    fast9_t(img, FAST9_THRESHOLD)
}

/// [`fast9`] with a caller-chosen threshold. Non-positive thresholds
/// are legal but meaningless (every flat region qualifies); the score
/// still pins each corner to its own maximum.
pub fn fast9_t(img: &Image<Gray, u8>, threshold: i32) -> Vec<Corner> {
    let (w, h) = (img.width(), img.height());
    if w < 7 || h < 7 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for y in 3..h - 3 {
        for x in 3..w - 3 {
            let Some(r) = ring(img, x, y) else { continue };
            let p = i32::from(img.as_slice()[(y * w + x) as usize]);
            let score = best_score(&r, p);
            if score > threshold {
                out.push(Corner { x, y, score });
            }
        }
    }
    out
}

/// Non-maximum suppression over the 8-connected neighbourhood: a corner
/// survives only if no detected neighbour carries a higher score, and
/// equal-score neighbours defer to the row-major first one. Runs on an
/// already-sorted (row-major) corner list and returns the subset in the
/// same order.
///
/// `corners` is what [`fast9`] returns — every detected corner, not a
/// pre-filtered subset, or suppression against invisible competitors
/// silently changes the survivors.
pub fn nonmax_suppress(corners: &[Corner]) -> Vec<Corner> {
    let mut keep = Vec::with_capacity(corners.len());
    'outer: for (i, c) in corners.iter().enumerate() {
        for (j, d) in corners.iter().enumerate() {
            if i == j {
                continue;
            }
            if (c.x as i64 - d.x as i64).abs() <= 1 && (c.y as i64 - d.y as i64).abs() <= 1 {
                // Neighbour wins ties by appearing earlier in row-major order.
                if d.score > c.score || (d.score == c.score && j < i) {
                    continue 'outer;
                }
            }
        }
        keep.push(*c);
    }
    keep
}

/// Score-descending sort with deterministic row-major tie-breaking —
/// the "keep the strongest `max` corners" step of the ORB recipe.
pub fn top_corners(corners: &mut Vec<Corner>, max: usize) {
    corners.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| (a.y, a.x).cmp(&(b.y, b.x)))
    });
    corners.truncate(max);
}

/// The patch intensity-centroid orientation at `(cx, cy)`: the angle of
/// the vector from the patch centre to its grey-value centroid over the
/// full 31×31 patch. `None` when any part of the patch is outside the
/// image.
///
/// `f64::atan2` over the accumulated moments; degenerate patches
/// (uniform grey ⇒ `m01 == m10 == 0`) return `0.0` — any angle steers a
/// constant patch identically, so zero is the honest, deterministic pick.
pub fn patch_orientation(img: &Image<Gray, u8>, cx: u32, cy: u32) -> Option<f64> {
    let (cx, cy) = (cx as i32, cy as i32);
    let (mut m10, mut m01) = (0i64, 0i64);
    for dy in -PATCH_RADIUS..=PATCH_RADIUS {
        for dx in -PATCH_RADIUS..=PATCH_RADIUS {
            let v = i64::from(at(img, cx + dx, cy + dy)?);
            m10 += v * i64::from(dx);
            m01 += v * i64::from(dy);
        }
    }
    Some((m01 as f64).atan2(m10 as f64))
}

/// Sample the grey value `steer`ed `dx, dy` away from `(cx, cy)`.
#[inline]
fn sample_steered(
    img: &Image<Gray, u8>,
    cx: i32,
    cy: i32,
    dx: i32,
    dy: i32,
    cos: f64,
    sin: f64,
) -> Option<i32> {
    let rx = (dx as f64 * cos - dy as f64 * sin).round() as i32;
    let ry = (dx as f64 * sin + dy as f64 * cos).round() as i32;
    at(img, cx + rx, cy + ry)
}

/// The steered-BRIEF descriptor of the 31×31 patch centred at
/// `(cx, cy)`, oriented by [`patch_orientation`].
///
/// `None` when the patch — after rotation — would sample outside the
/// image: because every pair offset lies inside the radius-15 disk, a
/// 15-pixel margin is the whole requirement. Two calls on identical
/// pixels return identical 256 bits; the same physical feature seen at
/// a different rotation returns (within rounding) the same bits because
/// the test pattern is rotated by the measured orientation.
pub fn rbrief(img: &Image<Gray, u8>, cx: u32, cy: u32) -> Option<Brief256> {
    let theta = patch_orientation(img, cx, cy)?;
    let (cos, sin) = (theta.cos(), theta.sin());
    let (cx, cy) = (cx as i32, cy as i32);
    let mut out = [0u8; 32];
    for (i, &((x1, y1), (x2, y2))) in BRIEF_PAIRS.iter().enumerate() {
        let a = sample_steered(img, cx, cy, i32::from(x1), i32::from(y1), cos, sin)?;
        let b = sample_steered(img, cx, cy, i32::from(x2), i32::from(y2), cos, sin)?;
        if a < b {
            out[i / 8] |= 0x80 >> (i % 8);
        }
    }
    Some(Brief256(out))
}

/// One ORB feature: the keypoint plus its descriptor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orb {
    /// Keypoint column.
    pub x: u32,
    /// Keypoint row.
    pub y: u32,
    /// Patch orientation, radians in `(−π, π]`.
    pub angle: f64,
    /// 256-bit rBRIEF descriptor.
    pub desc: Brief256,
}

/// The spec §4.3 ORB pipeline on a grey image: [`fast9`] →
/// [`nonmax_suppress`] → strongest `max_features` → [`rbrief`].
///
/// Corners whose 31×31 patch does not fit are dropped. `max_features`
/// of `0` yields an empty vector (a legal answer for flat or tiny
/// images).
pub fn orb(img: &Image<Gray, u8>, max_features: usize) -> Vec<Orb> {
    if max_features == 0 {
        return Vec::new();
    }
    let mut corners = nonmax_suppress(&fast9(img));
    top_corners(&mut corners, max_features);
    let mut out = Vec::with_capacity(corners.len());
    for c in corners {
        if let Some(desc) = rbrief(img, c.x, c.y) {
            let angle = patch_orientation(img, c.x, c.y).unwrap_or(0.0);
            out.push(Orb {
                x: c.x,
                y: c.y,
                angle,
                desc,
            });
        }
    }
    out
}
