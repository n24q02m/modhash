//! Deterministic RANSAC over 3-point affine fits — the geometric
//! verification step of spec §4.3: sample three point correspondences,
//! solve the six-parameter affine transform with
//! [`modhash_math::solve3`], count inliers under a pixel threshold, and
//! keep the model with the most support.
//!
//! The pinned constants (spec §4.3):
//!
//! * [`INLIER_PX`] = 4 px — the distance a projected source point must
//!   stay under to vote for a model.
//! * [`ITERATIONS`] = 500 — the sample count, run against a fixed-seed
//!   splitmix64 stream so identical inputs always yield identical
//!   models (the reference implementation's OpenCV RANSAC measured
//!   65 % vs 69 % on the same pair purely from RNG drift; this crate
//!   removes that nondeterminism entirely).
//!
//! The recovered [`Affine`] is a least-commitment object: it is the
//! best *sampled* triplet's solution, deliberately not re-fit to the
//! inlier set — the spec fixes the minimal-solve shape, and the noisy
//! inlier tolerance makes the difference immaterial at this precision.

use modhash_math::solve3;

use crate::Error;
use crate::prng::splitmix64_next;

/// Inlier distance threshold in pixels (spec §4.3).
pub const INLIER_PX: f64 = 4.0;

/// RANSAC iterations per estimate (spec §4.3).
pub const ITERATIONS: usize = 500;

/// A 2D affine transform `dst = M · src` in homogeneous form:
/// `(x', y') = (m00·x + m01·y + m02, m10·x + m11·y + m12)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    /// Row-major `[[m00, m01, m02], [m10, m11, m12]]`.
    pub m: [[f64; 3]; 2],
}

impl Affine {
    /// Apply the transform to `(x, y)`.
    pub fn project(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        (
            m[0][0] * x + m[0][1] * y + m[0][2],
            m[1][0] * x + m[1][1] * y + m[1][2],
        )
    }

    /// The rotation component of the transform, radians in `(−π, π]`.
    ///
    /// For a similarity/rotation-scale model `[[a, −b], [b, a]]` the
    /// angle is `atan2(m10 − m01, m00 + m11)`; for a general affine it
    /// is the rotation of the closest similarity, which is what "the
    /// rotation between two descriptor sets" means under the spec's
    /// usage. Pure reflections give the angle of the rotoreflection.
    pub fn rotation(&self) -> f64 {
        (self.m[1][0] - self.m[0][1]).atan2(self.m[0][0] + self.m[1][1])
    }

    /// The mean isotropic scale `√|det A|` of the linear part — `1.0`
    /// for a pure rotation.
    pub fn scale(&self) -> f64 {
        (self.m[0][0] * self.m[1][1] - self.m[0][1] * self.m[1][0])
            .abs()
            .sqrt()
    }
}

/// The winning model and its support.
#[derive(Clone, Debug)]
pub struct Estimate {
    /// Best sampled affine transform.
    pub model: Affine,
    /// Indices into the input slices that agree with `model` within
    /// `inlier_px`.
    pub inliers: Vec<usize>,
    /// Mean inlier reprojection error in pixels — the fit quality that
    /// breaks inlier-count ties deterministically.
    pub mean_error: f64,
}

/// Solve the exact affine map through three point pairs.
///
/// The row transform solves `[x y 1]·(m00 m01 m02)ᵀ = x'` and the
/// column likewise — two [`solve3`] calls sharing one coefficient
/// matrix. A collinear or repeated triplet is singular and returns
/// `None` (data, not an error — the sampler retries).
fn fit3(src: &[[f64; 2]], dst: &[[f64; 2]], triplet: [usize; 3]) -> Option<Affine> {
    let a: [[f64; 3]; 3] = [
        [src[triplet[0]][0], src[triplet[0]][1], 1.0],
        [src[triplet[1]][0], src[triplet[1]][1], 1.0],
        [src[triplet[2]][0], src[triplet[2]][1], 1.0],
    ];
    let bx = [dst[triplet[0]][0], dst[triplet[1]][0], dst[triplet[2]][0]];
    let by = [dst[triplet[0]][1], dst[triplet[1]][1], dst[triplet[2]][1]];
    let rx = solve3(&a, &bx)?;
    let ry = solve3(&a, &by)?;
    Some(Affine { m: [rx, ry] })
}

/// Deterministic RANSAC: pick the 3-point affine transform supported by
/// the most correspondences.
///
/// `src[i]` is the source keypoint, `dst[i]` its putative match; the
/// slices must be equal length ≥ 3 (`Error::BadValue` otherwise —
/// mismatched correspondence lists are a caller bug, not data).
/// Sampling draws triplets from a splitmix64 stream seeded with `seed`;
/// degenerate triplets (singular `solve3`) consume an iteration without
/// producing a candidate. `Error::NoFit` means all `iterations` draws
/// were degenerate — possible on tiny inputs, vanishingly unlikely
/// otherwise.
///
/// With `iterations == 0` no sampling happens and the answer is
/// `Error::NoFit`; pass [`ITERATIONS`] for the spec geometry.
pub fn ransac_affine(
    src: &[[f64; 2]],
    dst: &[[f64; 2]],
    inlier_px: f64,
    iterations: usize,
    seed: u64,
) -> Result<Estimate, Error> {
    if src.len() != dst.len() || src.len() < 3 {
        return Err(Error::BadValue(
            "ransac needs equal-length src/dst with at least 3 points",
        ));
    }
    let n = src.len();
    let thresh2 = inlier_px * inlier_px;
    let mut state = seed;
    let mut best: Option<Estimate> = None;

    for _ in 0..iterations {
        // Three distinct indices; rejection sampling is sound because
        // n ≥ 3 makes the draw space non-empty.
        let mut triplet = [0usize; 3];
        let mut fills = 0usize;
        let mut attempts = 0usize;
        while fills < 3 && attempts < 64 {
            attempts += 1;
            let i = (splitmix64_next(&mut state) % n as u64) as usize;
            if !triplet[..fills].contains(&i) {
                triplet[fills] = i;
                fills += 1;
            }
        }
        if fills < 3 {
            continue; // n just above 3, unlucky draws — keep sampling
        }
        let Some(model) = fit3(src, dst, triplet) else {
            continue;
        };

        let mut inliers = Vec::new();
        let mut err_sum = 0.0;
        for (i, (&s, &d)) in src.iter().zip(dst.iter()).enumerate() {
            let (px, py) = model.project(s[0], s[1]);
            let e2 = (px - d[0]) * (px - d[0]) + (py - d[1]) * (py - d[1]);
            if e2 <= thresh2 {
                inliers.push(i);
                err_sum += e2.sqrt();
            }
        }
        let mean_error = if inliers.is_empty() {
            f64::INFINITY
        } else {
            err_sum / inliers.len() as f64
        };

        let better = match &best {
            None => true,
            Some(b) => {
                inliers.len() > b.inliers.len()
                    || (inliers.len() == b.inliers.len() && mean_error < b.mean_error)
            }
        };
        if better {
            best = Some(Estimate {
                model,
                inliers,
                mean_error,
            });
        }
    }

    best.ok_or(Error::NoFit)
}
