//! Small dense linear algebra on `[[f64; 3]; 3]` — the matrix size the
//! tier-3 code actually carries (homographies, color transforms, the
//! point-cloud solves that feed [`crate::solve3`]).
//!
//! Everything here is a free function over plain arrays: no matrix
//! type, no allocation, no `unsafe`. Chaining is `mat3_mul(a, b)` and
//! friends; the return-by-value signature keeps expressions readable
//! (`mat3_mul_vec(&rot, &p)`) without a builder.
//!
//! The singularity convention matches [`crate::solve3`]: a determinant
//! is "zero" when `|det| <= ε·s³` where `s` is the largest `|m[i][j]|`.
//! The cube of the scale is the dimensionally honest threshold — a
//! determinant is a volume, so its magnitude scales like the cube of
//! the entries — and it keeps "singular" consistent between solving
//! and inverting: `solve3` refusing and [`inverse3`] refusing describe
//! the same set of matrices.

/// The 3×3 identity matrix.
pub const IDENTITY3: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Largest `|m[i][j]|` in the matrix — the scale every relative
/// tolerance in this crate measures against. Zero for the zero matrix.
fn max_abs3(m: &[[f64; 3]; 3]) -> f64 {
    m.iter()
        .flat_map(|row| row.iter())
        .fold(0.0_f64, |acc, &v| acc.max(v.abs()))
}

/// Determinant of `m`, computed by direct cofactor expansion along the
/// first row. For a 3×3 the expansion is exact and branchless; Gaussian
/// elimination would only complicate it.
#[must_use]
pub fn det3(m: &[[f64; 3]; 3]) -> f64 {
    let a = m[0];
    let b = m[1];
    let c = m[2];
    // a·(ei − fh) − b·(di − fg) + c·(dh − eg), grouped so each term is
    // one fused multiply-add where the hardware offers it.
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0])
}

/// Matrix product `a·b` (row-major, `c[i][j] = Σ_k a[i][k]·b[k][j]`).
#[must_use]
pub fn mat3_mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut c = [[0.0_f64; 3]; 3];
    for (ci, ai) in c.iter_mut().zip(a.iter()) {
        for (j, cij) in ci.iter_mut().enumerate() {
            *cij = ai[0].mul_add(b[0][j], ai[1].mul_add(b[1][j], ai[2] * b[2][j]));
        }
    }
    c
}

/// Matrix-vector product `m·v` (`out[i] = Σ_j m[i][j]·v[j]`).
#[must_use]
pub fn mat3_mul_vec(m: &[[f64; 3]; 3], v: &[f64; 3]) -> [f64; 3] {
    let mut out = [0.0_f64; 3];
    for (o, row) in out.iter_mut().zip(m.iter()) {
        *o = row[0].mul_add(v[0], row[1].mul_add(v[1], row[2] * v[2]));
    }
    out
}

/// Transpose of `m` (`out[i][j] = m[j][i]`).
#[must_use]
pub fn transpose3(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

/// Inverse of `m`, or `None` when `m` is singular or too close to
/// singular to invert meaningfully (`|det| <= ε·s³`, see the module
/// note). Computed via the adjugate: `m⁻¹ = adj(m) / det(m)`.
#[must_use]
pub fn inverse3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let s = max_abs3(m);
    if s == 0.0 {
        return None;
    }
    let det = det3(m);
    if det.is_nan() || det.abs() <= f64::EPSILON * s * s * s {
        return None;
    }
    let inv_det = 1.0 / det;
    let (a, b, c) = (m[0], m[1], m[2]);
    // adj(m) is the transpose of the cofactor matrix; written out so
    // each cofactor's sign alternation is visible in the literals.
    let adj = [
        [
            b[1] * c[2] - b[2] * c[1],
            a[2] * c[1] - a[1] * c[2],
            a[1] * b[2] - a[2] * b[1],
        ],
        [
            b[2] * c[0] - b[0] * c[2],
            a[0] * c[2] - a[2] * c[0],
            a[2] * b[0] - a[0] * b[2],
        ],
        [
            b[0] * c[1] - b[1] * c[0],
            a[1] * c[0] - a[0] * c[1],
            a[0] * b[1] - a[1] * b[0],
        ],
    ];
    let mut inv = [[0.0_f64; 3]; 3];
    for (inv_row, adj_row) in inv.iter_mut().zip(adj.iter()) {
        for (o, &a_v) in inv_row.iter_mut().zip(adj_row.iter()) {
            *o = a_v * inv_det;
        }
    }
    Some(inv)
}
