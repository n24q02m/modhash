//! Solving `A·x = b` for a 3×3 real system — the size the kit's RANSAC
//! homography step needs and the only size worth a dedicated routine.
//!
//! The method is Gaussian elimination with **partial pivoting** on an
//! augmented 3×4 matrix: at each column the row with the largest
//! `|m[row][col]|` becomes the pivot, so the algorithm is stable exactly
//! where the naive diagonal-first order divides by a near-zero. Three
//! columns means the whole thing is a fixed ~9-flop loop nest with zero
//! allocation — LAPACK it is not, and does not need to be.
//!
//! Singularity is reported as `None`, chosen over an error enum because
//! a singular or degenerate triplet of RANSAC points is ordinary data,
//! not a failure: the sampler simply draws another triplet.
//!
//! **The singularity test is scale-relative and documented.** A column
//! is treated as singular when its best pivot satisfies
//! `|pivot| <= ε·s` where `s` is the largest `|A[i][j]|` in the matrix
//! (an all-zero matrix has `s = 0` and is singular immediately). An
//! exact `== 0.0` test would pass pivot `1e-30` through elimination on a
//! matrix of unit-scale entries and return garbage confidently; the
//! `ε·s` threshold refuses pivots whose remaining precision cannot
//! carry a meaningful quotient, at the documented cost of calling a
//! borderline-conditioned matrix singular. For a kit that solves
//! pixel-coordinate systems this is the right side to err on.

/// Solves `a·x = b` for `x`, returning `None` when `a` is singular or
/// too close to singular to trust (see the module note for the exact
/// `|pivot| <= ε·s` rule). `a` is row-major `a[i][j]`, `x` and `b` are
/// length-3 vectors. Neither input is modified.
#[must_use]
pub fn solve3(a: &[[f64; 3]; 3], b: &[f64; 3]) -> Option<[f64; 3]> {
    let s = a
        .iter()
        .flat_map(|row| row.iter())
        .fold(0.0_f64, |acc, &v| acc.max(v.abs()));
    if s == 0.0 {
        // All-zero matrix: singular unless b is also zero, and even then
        // the solution is underdetermined — None either way.
        return None;
    }
    // Augmented 3×4 working copy; inputs are never touched.
    let mut m = [[0.0_f64; 4]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        row[..3].copy_from_slice(&a[i]);
        row[3] = b[i];
    }
    let tol = f64::EPSILON * s;
    for col in 0..3 {
        // Partial pivot: the largest |m[row][col]| on or below the
        // diagonal. A NaN pivot is treated as singular explicitly —
        // `abs() <= tol` is false for NaN, which would let it through
        // elimination and silently poison every quotient.
        let mut pivot = col;
        for (row, mrow) in m.iter().enumerate().skip(col) {
            if mrow[col].abs() > m[pivot][col].abs() {
                pivot = row;
            }
        }
        if m[pivot][col].is_nan() || m[pivot][col].abs() <= tol {
            return None;
        }
        m.swap(col, pivot);
        // Eliminate below the pivot. The pivot row stays unnormalized;
        // back-substitution divides once at the end. Copying the pivot
        // row out lets iter_mut borrow the rest cleanly.
        let pivot_row = m[col];
        for row in m.iter_mut().skip(col + 1) {
            let f = row[col] / pivot_row[col];
            row[col] = 0.0;
            for j in (col + 1)..4 {
                row[j] = pivot_row[j].mul_add(-f, row[j]);
            }
        }
    }
    // Back-substitution on the upper-triangular system.
    let mut x = [0.0_f64; 3];
    for i in (0..3).rev() {
        let mut acc = m[i][3];
        for (j, &mij) in m[i].iter().enumerate().take(3).skip(i + 1) {
            acc = mij.mul_add(-x[j], acc);
        }
        x[i] = acc / m[i][i];
    }
    Some(x)
}
