//! FastCDC content-defined chunking with the 16-level gear mask table.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! The algorithm is the one described by Wen Xia et al., "FastCDC: a Fast
//! and Efficient Content-Defined Chunking Approach for Data Deduplication",
//! USENIX ATC '16 (and its TPDS 2020 journal extension): a byte is a cut
//! point when `gear_fingerprint & mask == 0`, where the fingerprint rolls
//! one byte per step through the 256-entry [`GEAR`] table and `mask` is
//! chosen from [`MASKS`] by the target average size and the normalization
//! level. The constants are transcribed verbatim in
//! `docs/algorithms/fastcdc.md` and pinned bit-for-bit by `tests/tables.rs`;
//! a single flipped bit in either table changes every boundary downstream.
//!
//! Two rolling hashes live here because the paper only needs one of them:
//!
//! * [`Gear`], the `fp = (fp << 1) + Gear[b]` rolling fingerprint the
//!   chunker itself uses. A byte falls out of the window on its own once
//!   it has been shifted off the 64-bit end, so no eviction term exists.
//! * [`Buzhash64`], the classic rotate-XOR rolling hash over an explicit
//!   64-byte window, exported for chunk fingerprinting and other callers
//!   that want a real sliding window whose contents can be evicted.
//!
//! ```
//! use modhash_fastcdc::{FastCdc, Normalization};
//!
//! let data = vec![0xabu8; 1 << 16];
//! let chunker = FastCdc::new(&data, 2048, 8192, 32768).unwrap();
//! let total: usize = chunker.map(|c| c.length).sum();
//! assert_eq!(total, data.len());
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod buzhash;
mod cdc;
mod gear;
mod masks;

pub use crate::buzhash::{BUZHASH_WINDOW, Buzhash64};
pub use crate::cdc::{
    AVERAGE_MAX, AVERAGE_MIN, Chunk, FastCdc, MAXIMUM_MAX, MAXIMUM_MIN, MINIMUM_MAX, MINIMUM_MIN,
    Params, chunk,
};
pub use crate::gear::{GEAR, Gear};
pub use crate::masks::{MASKS, Normalization};
