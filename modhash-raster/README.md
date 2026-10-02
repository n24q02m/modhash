# `modhash-raster`

Raw image buffers, box-average resampling, BT.601 luma and EXIF orientation.

**Status: implemented.** Tight row-major `Image<L, T>` buffers (`Gray`,
`Rgb`, `Rgba` layouts; `u8`/`u16` samples), `box_average` resampling with
integer floor boundaries and `round_half_up`, BT.601 luma, and EXIF
orientation 1–8 via the eight public dihedral transforms. Conventions are
pinned in `docs/algorithms/raster.md`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: none.
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
