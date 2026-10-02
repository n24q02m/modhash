//! The numeric kernel of the `modhash` kit: Fourier transforms, the
//! DCT, order statistics, and the smallest linear algebra that tier 3
//! needs.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in
//! this workspace builds without a single registry package, enforced by
//! `scripts/gate_zero_dep.sh`, and the workspace ordering
//! (`scripts/gate_dag.py`) makes this crate #3 — above
//! `modhash-primitives` and `modhash-inflate`, below every decoder.
//!
//! The crate is `std`, not `no_std`: the FFT twiddle factors and the
//! DCT kernel need `f64::sin`/`f64::cos`, which `core` does not provide.
//! It still allocates only through `Vec` at API boundaries — all
//! in-place transforms keep working-set memory caller-owned.
//!
//! # The pinned conventions
//!
//! * [`median`]: for even `n` the result is the **lower** middle
//!   element `sorted[n/2 - 1]`, never the mean of the two middles. The
//!   module docs carry the argument; the test on `[1,2,3,4]` pins it.
//! * [`fft`]/[`ifft`]: forward uses `e^(−2πi/N)`, inverse conjugates —
//!   `ifft(fft(x)) == x` exactly, no 1/N on the forward path.
//! * [`dct2`]/[`idct2`]: **orthonormal** DCT-II/DCT-III, so the inverse
//!   is the transposed kernel and `idct2(dct2(x)) == x`.
//! * [`solve3`]/[`inverse3`]: singularity is scale-relative,
//!   `pivot <= ε·s` / `|det| <= ε·s³`, and reported as `None` — a
//!   degenerate RANSAC triplet is data, not an error.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod dct;
mod fft;
mod linalg;
mod median;
mod solve3;

pub use dct::{dct2, dct2_2d, dct2_in_place, idct2, idct2_2d};
pub use fft::{Complex, fft, fft_real, ifft};
pub use linalg::{IDENTITY3, det3, inverse3, mat3_mul, mat3_mul_vec, transpose3};
pub use median::{median, median_copy};
pub use solve3::solve3;
