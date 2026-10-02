//! The normalized-chunking FastCDC scan and its public `chunk` API.
//!
//! This is Algorithm 1 of the 2016 paper ("FastCDC8KB"), generalised over
//! `min/avg/max` and the [`Normalization`] level:
//!
//! 1. **Sub-minimum cut-point skipping:** positions before `min_size` are
//!    never hash-judged; scanning starts at `min_size`.
//! 2. **Normalized chunking:** positions before the average are judged with
//!    `mask_s` (more one-bits — cuts are rarer, chunks grow toward the
//!    average) and positions at or beyond the average with `mask_l` (fewer
//!    one-bits — cuts come sooner). See [`crate::masks`].
//! 3. **Zero-padded hash judgment:** the test is `fp & mask == 0` — no
//!    threshold comparison — and the mask's spread one-bits widen the
//!    effective fingerprint window.
//!
//! The fingerprint resets to zero at every cut, matching the reference
//! implementation (`fingerprint = 0` at the top of each chunking call).
//! Chunk lengths returned therefore satisfy `min_size <= len <= max_size`
//! for every chunk except possibly the last, which may be shorter.

use alloc::vec::Vec;
use modhash_primitives::{Error, Result};

use crate::gear::gear_update;
use crate::masks::{Normalization, logarithm2, select_masks};

/// Smallest accepted `min_size`: the table's first real level is 64 B.
pub const MINIMUM_MIN: usize = 64;
/// Largest accepted `min_size`: 1 MiB.
pub const MINIMUM_MAX: usize = 1_048_576;
/// Smallest accepted `avg_size`: 256 B.
pub const AVERAGE_MIN: usize = 256;
/// Largest accepted `avg_size`: 4 MiB.
pub const AVERAGE_MAX: usize = 4_194_304;
/// Smallest accepted `max_size`: 1 KiB.
pub const MAXIMUM_MIN: usize = 1024;
/// Largest accepted `max_size`: 16 MiB.
pub const MAXIMUM_MAX: usize = 16_777_216;

/// Validated `(min_size, avg_size, max_size, level)` parameters.
///
/// The bounds keep `avg_size`'s mask index and its `± level` neighbours
/// inside [`crate::MASKS`]: `avg_size` in `256 B..=4 MiB` gives
/// `round(log2(avg))` in `8..=22`, and level 3 reaches indices `5..=25`,
/// which is exactly the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Params {
    /// Sub-minimum chunk cut-point skip distance: no position before this
    /// offset inside a chunk is ever hash-judged.
    pub min_size: usize,
    /// Target average chunk size: selects the mask level and the point
    /// where judgment switches from the strict to the loose mask.
    pub avg_size: usize,
    /// Hard cap on a chunk's length; scanning never looks past it.
    pub max_size: usize,
    /// Normalization level; [`Normalization::Level2`] is the default.
    pub level: Normalization,
}

impl Params {
    /// Validates the four parameters, returning [`Error::BadValue`] naming
    /// the offending field.
    pub fn validate(self) -> Result<Params> {
        if !(MINIMUM_MIN..=MINIMUM_MAX).contains(&self.min_size) {
            return Err(Error::BadValue("min_size outside 64..=1 MiB"));
        }
        if !(AVERAGE_MIN..=AVERAGE_MAX).contains(&self.avg_size) {
            return Err(Error::BadValue("avg_size outside 256..=4 MiB"));
        }
        if !(MAXIMUM_MIN..=MAXIMUM_MAX).contains(&self.max_size) {
            return Err(Error::BadValue("max_size outside 1 KiB..=16 MiB"));
        }
        if self.min_size > self.avg_size {
            return Err(Error::BadValue("min_size > avg_size"));
        }
        if self.avg_size > self.max_size {
            return Err(Error::BadValue("avg_size > max_size"));
        }
        // Guard the mask index arithmetic: bits + level and bits - level
        // must stay inside MASKS. The size bounds above already imply
        // bits in 8..=22 and level <= 3, so this is a second line of
        // defence, not the primary check.
        let bits = logarithm2(self.avg_size) as usize;
        let level = self.level.bits() as usize;
        if bits + level >= crate::masks::MASKS.len() || bits < level + 5 {
            return Err(Error::BadValue(
                "avg_size/level select a mask outside the table",
            ));
        }
        Ok(self)
    }
}

/// A chunk returned by [`FastCdc`]: an `(offset, length)` span plus the
/// Gear fingerprint value at the moment the cut was declared.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Chunk {
    /// Gear fingerprint as of the end of the chunk; useful for chunk-size
    /// prediction schemes (RapidCDC-style) and cheap identity checks. It
    /// has low entropy — it is a rolling hash, not a digest.
    pub hash: u64,
    /// Byte offset of the chunk start within the source.
    pub offset: usize,
    /// Chunk length in bytes.
    pub length: usize,
}

/// Finds the next cut point in `source[start..]` and returns the chunk
/// length and the fingerprint at the cut.
///
/// Faithful to the paper's Algorithm 1, including its boundary convention:
/// the byte whose insertion satisfies `fp & mask == 0` at index `i` becomes
/// the *first byte of the next chunk*, so the returned length is `i`
/// (positions scanned are `min_size .. remaining`, and a cut at `i` covers
/// `source[..i]`). Callers that need the paper's `i + 1` convention can add
/// one to the length without affecting determinism.
fn cut(source: &[u8], params: Params, mask_s: u64, mask_l: u64) -> (u64, usize) {
    let mut remaining = source.len();
    if remaining <= params.min_size {
        return (0, remaining);
    }
    // The hash judgment splits at the average size — unless the input
    // itself ends sooner, in which case the loose-mask region is empty.
    let mut center = params.avg_size;
    if remaining > params.max_size {
        remaining = params.max_size;
    } else if remaining < center {
        center = remaining;
    }
    let mut fp: u64 = 0;
    let mut i = params.min_size;
    // Strict mask below the average: harder to cut, pushes sizes up.
    while i < center {
        fp = gear_update(fp, source[i]);
        if fp & mask_s == 0 {
            return (fp, i);
        }
        i += 1;
    }
    // Loose mask from the average on: easier to cut, pulls sizes down.
    while i < remaining {
        fp = gear_update(fp, source[i]);
        if fp & mask_l == 0 {
            return (fp, i);
        }
        i += 1;
    }
    // Pathological inputs (e.g. all-zero data) never satisfy the judgment:
    // fall back to the maximum length.
    (fp, remaining)
}

/// Iterator over the content-defined chunks of a byte slice.
///
/// Created by [`FastCdc::new`] / [`FastCdc::with_level`]. Each step calls
/// `cut` on the remainder, so `N` chunks cost `N` linear scans of the
/// bytes they cover — O(source length) overall.
#[derive(Clone, Debug)]
pub struct FastCdc<'a> {
    source: &'a [u8],
    params: Params,
    mask_s: u64,
    mask_l: u64,
    /// Absolute offset of the next chunk's first byte.
    offset: usize,
}

impl<'a> FastCdc<'a> {
    /// Chunks `source` at the default [`Normalization::Level2`].
    ///
    /// `min_size`, `avg_size` and `max_size` are validated by
    /// [`Params::validate`]; the spec's defaults are `2048 / 8192 / 32768`.
    pub fn new(
        source: &'a [u8],
        min_size: usize,
        avg_size: usize,
        max_size: usize,
    ) -> Result<FastCdc<'a>> {
        Self::with_level(
            source,
            min_size,
            avg_size,
            max_size,
            Normalization::default(),
        )
    }

    /// Chunks `source` with an explicit normalization level.
    pub fn with_level(
        source: &'a [u8],
        min_size: usize,
        avg_size: usize,
        max_size: usize,
        level: Normalization,
    ) -> Result<FastCdc<'a>> {
        let params = Params {
            min_size,
            avg_size,
            max_size,
            level,
        }
        .validate()?;
        let (mask_s, mask_l) = select_masks(avg_size, level);
        Ok(FastCdc {
            source,
            params,
            mask_s,
            mask_l,
            offset: 0,
        })
    }

    /// The validated parameters this chunker runs with.
    pub const fn params(&self) -> Params {
        self.params
    }
}

impl Iterator for FastCdc<'_> {
    type Item = Chunk;

    fn next(&mut self) -> Option<Chunk> {
        let remaining = self.source.len().saturating_sub(self.offset);
        if remaining == 0 {
            return None;
        }
        let (hash, length) = cut(
            &self.source[self.offset..],
            self.params,
            self.mask_s,
            self.mask_l,
        );
        // `cut` never returns a length past the slice it was given.
        debug_assert!(length >= 1 && length <= remaining);
        let chunk = Chunk {
            hash,
            offset: self.offset,
            length,
        };
        self.offset += length;
        Some(chunk)
    }
}

/// Convenience wrapper: runs [`FastCdc`] over `data` and collects the
/// chunk boundaries as `(offset, length)` pairs.
///
/// ```
/// use modhash_fastcdc::chunk;
///
/// // Chunks always tile the input exactly.
/// let data = vec![7u8; 10 * 1024];
/// let bounds = chunk(&data, 2048, 4096, 8192).unwrap();
/// let total: usize = bounds.iter().map(|&(_, len)| len).sum();
/// assert_eq!(total, data.len());
/// assert_eq!(bounds[0].0, 0);
/// ```
pub fn chunk(
    data: &[u8],
    min_size: usize,
    avg_size: usize,
    max_size: usize,
) -> Result<Vec<(usize, usize)>> {
    FastCdc::new(data, min_size, avg_size, max_size)
        .map(|it| it.map(|c| (c.offset, c.length)).collect())
}
