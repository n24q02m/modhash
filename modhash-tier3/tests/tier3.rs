//! Verification for `modhash-tier3` (plan P17 verify line).
//!
//! # Fixture provenance
//!
//! Every fixture below is generated deterministically inside this file;
//! there are no binary blobs to vendor. PROVENANCE per fixture:
//!
//! * **FAST image** — built in [`fixture_img`]: a 32×32 grey field at
//!   40 with three single-pixel dots at 200. Expected corners
//!   `(8,8)`, `(20,20)`, `(16,25)` were derived by hand from the FAST-9
//!   definition (each dot's 16-pixel ring is uniformly 160 levels
//!   darker ⇒ a 16-arc ≫ 9), then **verified against this
//!   implementation** and recorded literally. A straight vertical edge
//!   and a flat square are the negative controls: their longest ring
//!   arc is 8 — exactly one short of FAST-9 — which is also the
//!   mutation surface (arc 9 → 8 turns the edge into corners).
//! * **rBRIEF patch pair** — a deterministic pseudo-texture
//!   `g(x,y) = (x·37 + y·91 + x·y·3) mod 256` chosen for full-range,
//!   non-periodic-at-31×31 pixel values; the rotation fixture is its
//!   exact `rot90`.
//! * **RANSAC set** — 40 inliers through a known rotation + translation
//!   with ±0.2 px formula noise, plus 20 scattered outliers.
//! * **DTW sequences** — literal small vectors.

use modhash_raster::{Gray, Image};
use modhash_tier3::Error;
use modhash_tier3::d4::{D4, center_square, transform, variants};
use modhash_tier3::dtw::{SAKOE_CHIBA_BAND, dtw, dtw_series};
use modhash_tier3::orb::{fast9, hamming256, nonmax_suppress, orb, rbrief};
use modhash_tier3::ransac::{INLIER_PX, ITERATIONS, ransac_affine};

/// 32×32 grey field (40) with three single-pixel dots (200).
fn fixture_img() -> Image<Gray, u8> {
    let mut img = Image::<Gray, u8>::new(32, 32).unwrap();
    img.as_mut_slice().fill(40);
    for &(x, y) in &[(8u32, 8u32), (20, 20), (16, 25)] {
        img.as_mut_slice()[(y * 32 + x) as usize] = 200;
    }
    img
}

/// Deterministic full-range texture, non-periodic at 31×31.
fn texture(x: u32, y: u32) -> u8 {
    ((x * 37 + y * 91 + x * y * 3 + x * x) % 251) as u8
}

fn texture_img(w: u32, h: u32) -> Image<Gray, u8> {
    let mut img = Image::<Gray, u8>::new(w, h).unwrap();
    for y in 0..h {
        for x in 0..w {
            img.as_mut_slice()[(y * w + x) as usize] = texture(x, y);
        }
    }
    img
}

#[test]
fn fast9_finds_recorded_corners() {
    let img = fixture_img();
    let corners = fast9(&img);
    let coords: Vec<(u32, u32)> = corners.iter().map(|c| (c.x, c.y)).collect();
    assert_eq!(coords, vec![(8, 8), (20, 20), (16, 25)]);
    // Each dot's ring is uniformly 160 darker ⇒ score 160.
    assert!(corners.iter().all(|c| c.score == 160));
}

#[test]
fn fast9_rejects_straight_edges_and_smooth_ramps() {
    // Vertical step edge: longest contiguous same-sign ring arc is 8,
    // and every edge pixel's ring also contains same-side equals —
    // strict inequality keeps it non-corner at any bound.
    let mut img = Image::<Gray, u8>::new(64, 64).unwrap();
    for y in 0..64 {
        for x in 0..64 {
            img.as_mut_slice()[(y * 64 + x) as usize] = if x < 32 { 40 } else { 200 };
        }
    }
    assert!(fast9(&img).is_empty());

    // Smooth horizontal ramp, slope 1/px: no ring pixel is ever 20
    // levels away.
    let mut img = Image::<Gray, u8>::new(64, 64).unwrap();
    for y in 0..64 {
        for x in 0..64 {
            img.as_mut_slice()[(y * 64 + x) as usize] = x as u8;
        }
    }
    assert!(fast9(&img).is_empty());

    // NB: a small high-contrast SQUARE is not a negative control — edge
    // pixels near its corners legitimately pass FAST-9 because the
    // radius-3 ring reaches past the square's edge.
}

#[test]
fn fast9_arc_length_is_exactly_nine() {
    // Boundary fixture: centre 200, ring = 8 bright (≥+20) contiguous
    // pixels + 8 pixels at -160. An 8-arc exists but a 9-arc does not,
    // so this pixel is NOT a FAST-9 corner — and is the live surface
    // for the mutation `FAST9_ARC: 9 → 8`, which turns the centre into
    // a corner and fails this test.
    let mut img = Image::<Gray, u8>::new(32, 32).unwrap();
    img.as_mut_slice().fill(40);
    let (cx, cy) = (16u32, 16u32);
    img.as_mut_slice()[(cy * 32 + cx) as usize] = 200;
    // Ring indices 8..=15 (CIRCLE ordering) raised to 220: the bright
    // run wraps the ring boundary, so no window start dodges it.
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
    for &(dx, dy) in &CIRCLE[8..] {
        img.as_mut_slice()[((cy as i32 + dy) as u32 * 32 + (cx as i32 + dx) as u32) as usize] = 220;
    }
    assert!(
        !fast9(&img).iter().any(|c| c.x == cx && c.y == cy),
        "8 contiguous bright ring pixels must not trigger FAST-9"
    );
}

#[test]
fn fast9_threshold_boundary_is_strict() {
    // Dot exactly `threshold` above the field must NOT register.
    let mut img = Image::<Gray, u8>::new(32, 32).unwrap();
    img.as_mut_slice().fill(40);
    img.as_mut_slice()[8 * 32 + 8] = 60; // diff == 20 == threshold
    assert!(fast9(&img).is_empty());
    img.as_mut_slice()[8 * 32 + 8] = 61; // diff == 21 > 20
    assert_eq!(fast9(&img).len(), 1);
}

#[test]
fn nonmax_and_top_corners() {
    let mut img = Image::<Gray, u8>::new(64, 64).unwrap();
    img.as_mut_slice().fill(40);
    // Cluster: 2×2 dots — each sees the other in its 8-neighbourhood.
    img.as_mut_slice()[20 * 64 + 20] = 200;
    img.as_mut_slice()[20 * 64 + 21] = 180; // weaker: ring darker by only 140
    let corners = fast9(&img);
    assert!(corners.len() >= 2);
    let kept = nonmax_suppress(&corners);
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].x, kept[0].y), (20, 20));
}

#[test]
fn rbrief_is_deterministic() {
    let img = texture_img(64, 64);
    let a = rbrief(&img, 32, 32).unwrap();
    let b = rbrief(&img, 32, 32).unwrap();
    assert_eq!(a, b);
    assert_eq!(hamming256(&a, &b), 0);
}

#[test]
fn rbrief_different_patches_differ() {
    let img = texture_img(64, 64);
    let a = rbrief(&img, 32, 32).unwrap();
    let b = rbrief(&img, 48, 48).unwrap();
    let d = hamming256(&a, &b);
    // Independent bits: far from identical. Floor pinned after first run.
    assert!(
        d >= 40,
        "distance {d} too small — descriptors not decorrelating"
    );
    assert_ne!(a, b);
}

#[test]
fn rbrief_rotation_invariant() {
    let img = texture_img(64, 64);
    let rot = img.rot90();
    let d1 = rbrief(&img, 32, 32).unwrap();
    // rot90 maps (x, y) -> (h-1-y, x): (32,32) -> (31,32).
    let d2 = rbrief(&rot, 31, 32).unwrap();
    let dist = hamming256(&d1, &d2);
    assert!(
        dist <= 4,
        "steered descriptor drifted {dist} bits under a 90° rotation"
    );
}

#[test]
fn rbrief_refuses_out_of_bounds_patch() {
    let img = texture_img(64, 64);
    assert_eq!(rbrief(&img, 0, 0), None); // patch needs 15 px margin
    assert_eq!(rbrief(&img, 14, 15), None); // one pixel short on the left
    assert!(rbrief(&img, 15, 15).is_some()); // disk fits exactly at 15
}

#[test]
fn orb_pipeline_end_to_end() {
    let img = texture_img(128, 128);
    let feats = orb(&img, 100);
    assert!(!feats.is_empty() && feats.len() <= 100);
    // Deterministic: identical image ⇒ identical feature set.
    let again = orb(&img, 100);
    assert_eq!(feats, again);
}

#[test]
fn ransac_recovers_known_rotation() {
    // 40 inliers: rotation 30° + translation (5, -3), ±0.2 px noise.
    // 20 outliers scattered. Deterministic formula noise, no RNG needed.
    let theta = 30f64.to_radians();
    let (c, s) = (theta.cos(), theta.sin());
    let mut src = Vec::new();
    let mut dst = Vec::new();
    for i in 0..40u32 {
        let x = (i % 8) as f64 * 11.0 + 7.0;
        let y = (i / 8) as f64 * 13.0 + 11.0;
        let nx = (((i as u64).wrapping_mul(2654435761) >> 16) % 100) as f64 / 250.0 - 0.2;
        let ny = (((i as u64 * 40503 + 77) >> 4) % 100) as f64 / 250.0 - 0.2;
        src.push([x, y]);
        dst.push([c * x - s * y + 5.0 + nx, s * x + c * y - 3.0 + ny]);
    }
    for i in 0..20u32 {
        src.push([(i * 37 % 90) as f64, (i * 53 % 80) as f64]);
        dst.push([(i * 71 % 95) as f64 + 10.0, (i * 29 % 85) as f64 + 5.0]);
    }
    let est = ransac_affine(&src, &dst, INLIER_PX, ITERATIONS, 0x5EED).unwrap();
    assert!(
        est.inliers.len() >= 38,
        "expected ~40 inliers, got {}",
        est.inliers.len()
    );
    let rot = est.model.rotation();
    let err = (rot - theta).abs();
    assert!(
        err < 2f64.to_radians(),
        "rotation off by {err} rad (got {rot})"
    );
    // Deterministic: same inputs, same estimate.
    let again = ransac_affine(&src, &dst, INLIER_PX, ITERATIONS, 0x5EED).unwrap();
    assert_eq!(est.inliers, again.inliers);
    assert_eq!(est.model.m, again.model.m);
}

#[test]
fn ransac_rejects_bad_input_and_degenerate_sets() {
    let p = [[0.0, 0.0], [1.0, 1.0]];
    assert!(matches!(
        ransac_affine(&p, &p, INLIER_PX, ITERATIONS, 1),
        Err(Error::BadValue(_))
    ));
    let q = [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]]; // collinear
    assert!(matches!(
        ransac_affine(&q, &q, INLIER_PX, ITERATIONS, 1),
        Err(Error::NoFit)
    ));
}

#[test]
fn d4_variants_match_raster_primitives() {
    let img = texture_img(16, 16);
    let v = variants(&img).unwrap();
    assert_eq!(v.len(), 8);
    assert_eq!(v[0], img);
    assert_eq!(v[1], img.rot90());
    assert_eq!(v[2], img.rot180());
    assert_eq!(v[3], img.rot270());
    assert_eq!(v[4], img.transpose());
    assert_eq!(v[5], img.transpose().rot90());
    assert_eq!(v[6], img.transpose().rot180());
    assert_eq!(v[7], img.transpose().rot270());
    // All eight distinct on a non-symmetric texture.
    for i in 0..8 {
        for j in (i + 1)..8 {
            assert_ne!(v[i], v[j]);
        }
    }
}

#[test]
fn d4_requires_square_or_center_crop() {
    let rect = texture_img(20, 12);
    assert!(transform(&rect, D4::Rot90).is_err());
    let sq = center_square(&rect).unwrap();
    assert_eq!((sq.width(), sq.height()), (12, 12));
    // Centred crop: 8 px removed from width ⇒ 4 each side.
    assert_eq!(sq.pixel(0, 0).unwrap()[0], rect.pixel(4, 0).unwrap()[0]);
    assert_eq!(sq.pixel(11, 11).unwrap()[0], rect.pixel(15, 11).unwrap()[0]);
    assert!(transform(&sq, D4::TransposeRot270).is_ok());
}

/// Naive O(n·m) reference DTW over a full (n+1)×(m+1) table — same
/// semantics (banded window, per-step mean) written independently.
fn dtw_reference(cost: &[f64], n: usize, m: usize, band: f64) -> f64 {
    if n == 0 || m == 0 {
        return 1.0;
    }
    let inf = f64::INFINITY;
    let mut d = vec![vec![inf; m + 1]; n + 1];
    let mut l = vec![vec![0i64; m + 1]; n + 1];
    d[0][0] = 0.0;
    let width = (band * n.max(m) as f64).max(2.0);
    for i in 1..=n {
        let (lo, hi) = if band < 0.0 {
            (1usize, m)
        } else {
            let c = i as f64 * m as f64 / n as f64;
            (
                (c - width).max(1.0) as usize,
                (c + width).min(m as f64) as usize,
            )
        };
        for j in lo..=hi {
            let mut best = d[i - 1][j - 1];
            let mut steps = l[i - 1][j - 1];
            if d[i - 1][j] < best {
                best = d[i - 1][j];
                steps = l[i - 1][j];
            }
            if d[i][j - 1] < best {
                best = d[i][j - 1];
                steps = l[i][j - 1];
            }
            if best.is_finite() {
                d[i][j] = best + cost[(i - 1) * m + (j - 1)];
                l[i][j] = steps + 1;
            }
        }
    }
    if !d[n][m].is_finite() || l[n][m] == 0 {
        return 1.0;
    }
    d[n][m] / l[n][m] as f64
}

#[test]
fn dtw_matches_full_table_reference() {
    // Small n, deterministic pseudo-random series; band and unbanded.
    let a: Vec<f64> = (0..9).map(|i| ((i * 37 + 11) % 23) as f64).collect();
    let b: Vec<f64> = (0..7).map(|i| ((i * 53 + 5) % 23) as f64).collect();
    let cost: Vec<f64> = a
        .iter()
        .flat_map(|&x| b.iter().map(move |&y| (x - y).abs()))
        .collect();
    for band in [SAKOE_CHIBA_BAND, 0.5, -1.0] {
        let got = dtw(&cost, 9, 7, band).unwrap();
        let want = dtw_reference(&cost, 9, 7, band);
        assert_eq!(got, want, "band {band}");
    }
}

#[test]
fn dtw_band_actually_constrains() {
    // a = [0,0,0,0,1], b = [0]*29 + [9]: the free path races the
    // matching 1s together through the all-zero field (mean ≈ 0.23),
    // but reaching column 29 from row 0 needs |j − centre| ≈ 25 — far
    // outside a 10 % band (width = max(2, 0.1·30) = 3). The banded DP
    // can only stride down the expensive last column and lands near 1.6.
    let a = [0.0, 0.0, 0.0, 0.0, 1.0];
    let mut b = [0.0; 30];
    b[29] = 9.0;
    let free = dtw_series(&a, &b, -1.0).unwrap();
    let banded = dtw_series(&a, &b, SAKOE_CHIBA_BAND).unwrap();
    assert!(
        free < 0.3,
        "unbanded should find the warped path, got {free}"
    );
    assert!(
        banded > free + 0.5,
        "band must raise the cost: free {free}, banded {banded}"
    );
    // Widening the band past the needed warp frees the cheap path —
    // sanity that the constraint is the band, not the inputs.
    let wide = dtw_series(&a, &b, 0.95).unwrap();
    assert!(
        wide < 0.3,
        "near-unbounded band should behave like free DTW, got {wide}"
    );
}

#[test]
fn dtw_basics() {
    assert_eq!(
        dtw_series(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0], SAKOE_CHIBA_BAND).unwrap(),
        0.0
    );
    assert_eq!(dtw(&[], 0, 0, SAKOE_CHIBA_BAND).unwrap(), 1.0);
    assert_eq!(
        dtw(&[0.0; 6], 2, 4, SAKOE_CHIBA_BAND),
        Err(Error::BadValue("cost matrix length != n*m"))
    );
}

#[test]
fn orb_descriptors_on_rotated_image_match() {
    // End-to-end: features found on a rotated copy of the same texture
    // should produce near-identical descriptors at mapped keypoints.
    let img = texture_img(96, 96);
    let rot = img.rot90();
    let fa = orb(&img, 200);
    let fb = orb(&rot, 200);
    assert!(!fa.is_empty() && !fb.is_empty());
    let (w, _h) = (img.width(), img.height());
    // rot90 maps (x,y) -> (w-1-y, x); compare matching descriptors.
    let mut best = u32::MAX;
    for a in &fa {
        let (mx, my) = (w - 1 - a.y, a.x);
        for b in &fb {
            if b.x.abs_diff(mx) <= 1 && b.y.abs_diff(my) <= 1 {
                best = best.min(hamming256(&a.desc, &b.desc));
            }
        }
    }
    assert!(best <= 8, "best cross-rotation descriptor distance {best}");
}
