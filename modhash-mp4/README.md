# `modhash-mp4`

ISO base media file format demuxing, avcC records and sample tables.

**Status: implemented.** `demux` walks the box tree (`size==0`, `size==1`
largesize and `uuid` forms), resolves `moov → trak → mdia → minf → stbl`,
expands `stts`/`stsc`/`stsz`/`stco`/`co64`/`ctts`/`stss` into per-sample
records, and surfaces the `avcC` record of H.264 tracks. Fragmented MP4
(`moof`/`mvex`) is refused with `Error::Unsupported`.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.
