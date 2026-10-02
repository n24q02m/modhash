//! ISO base media file format demuxing, `avcC` records and sample tables
//! (ISO/IEC 14496-12; AVC codec configuration per ISO/IEC 14496-15 §5.3.3).
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! This crate **demuxes only**. It walks the ISO-BMFF box tree, resolves the
//! `moov → trak → mdia → minf → stbl` sample-table chain, and hands callers
//! an iteratable view of samples — offset, size, decoding/presentation time,
//! sync flag — plus the raw `avcC` record of H.264 tracks. It does not decode
//! video: turning `avcC` + sample bytes into frames is `modhash-h264`'s job.
//!
//! Fragmented MP4 (`moof`/`mvex`) is a real format variant a sample-table
//! demuxer cannot answer honestly, so it is refused with
//! `Error::Unsupported`; everything malformed or short fails
//! `Error::Truncated` or `Error::BadValue`, and no input can panic the
//! parser — every read goes through bounds-checked helpers.
//!
//! The crate is `no_std` apart from the `alloc` collections its API returns.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod boxes;
mod demux;
mod sample_table;

pub use crate::boxes::{BoxHead, Boxes, Four, Reader, read_box};
pub use crate::demux::{Mp4, Track, demux};
pub use crate::sample_table::{
    ChunkRun, CompRun, EntryKind, Sample, SampleDescription, SampleIter, SampleTable, TimeRun,
};
