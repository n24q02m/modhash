# `modhash-math`

FFT, DCT-II, median, 3x3 solve and small linear algebra.

**Status: implemented.** Complex + real FFT (radix-2, in place),
orthonormal DCT-II/DCT-III (1D and separable 2D — the direct O(N²)
definition, deliberately; see `src/dct.rs`), median (**lower-middle**
convention for even lengths, documented in `src/median.rs`), `solve3`
(partial-pivot Gaussian elimination), and 3×3 helpers (`det3`,
`mat3_mul`, `mat3_mul_vec`, `transpose3`, `inverse3`). The crate is
`std` (`sin`/`cos` are not in `core`) but allocates only through `Vec`
at API boundaries.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: none.
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
