//! The FastCDC mask table: one cut-point test mask per target chunk-size
//! level, plus the normalization-level selector.
//!
//! A candidate byte position is a cut point when `fp & mask == 0`. A mask
//! with `k` one-bits declares a cut with probability about `2^-k` per byte,
//! so `MASKS[i]` — which always carries exactly `i` one-bits — targets an
//! average chunk size of `2^i` bytes. The one-bits are spread over the
//! upper half of the word rather than packed into the low bits: that is the
//! paper's "zero-padded mask" trick, which stretches the effective sliding
//! window of the Gear fingerprint to roughly 48 bytes so it matches the
//! window Rabin-based CDC uses.
//!
//! `MASKS` is indexed directly by `round(log2(avg_size))`, so indices 0
//! through 5 are padding and 6 through 21 are the sixteen usable levels,
//! 64 B (`2^6`) through 2 MiB (`2^21`). Indices 22 through 25 extend the
//! table to 16 MiB; 5 and 25 exist so normalization level 3 still resolves
//! for the extreme table ends. Every entry below is transcribed verbatim in
//! `docs/algorithms/fastcdc.md` and pinned bit-for-bit in `tests/tables.rs`
//! — a wrong bit here is silent corruption, not a crash.

/// Cut-point test masks, one per target chunk-size level.
///
/// `MASKS[6]` targets 64 B, `MASKS[21]` targets 2 MiB: sixteen levels, the
/// table the design spec calls "bảng mask 16 mức". Entries outside that
/// span exist only to keep the index arithmetic total: `0..=4` are padding,
/// `22..=24` extend to 16 MiB, and 5/25 are only reachable at normalization
/// level 3 on the smallest and largest `avg_size` values.
///
/// Provenance: indices 6–17 match the `g_condition_mask` table in the
/// FastCDC authors' C reference implementation (the destor tree); the rest
/// continue the same spread-bit pattern. The three masks named in the 2016
/// paper's Algorithm 1 correspond to `MASKS[13]` (`MaskA`, 13 bits),
/// `MASKS[15]` (`MaskS`, 15 bits) and `MASKS[11]` (`MaskL`, 11 bits) for an
/// 8 KiB target — same bit counts, but deliberately different bit
/// placement, so do not "fix" them to the paper's literals.
pub const MASKS: [u64; 26] = [
    0x0000_0000_0000_0000, // padding
    0x0000_0000_0000_0000, // padding
    0x0000_0000_0000_0000, // padding
    0x0000_0000_0000_0000, // padding
    0x0000_0000_0000_0000, // padding
    0x0000_0000_0180_4110, // level -1 guard, only used at level 3 on 64B
    0x0000_0000_0180_3110, // 64 B
    0x0000_0000_1803_5100, // 128 B
    0x0000_0018_0003_5300, // 256 B
    0x0000_0190_0035_3000, // 512 B
    0x0000_5900_0353_0000, // 1 KiB
    0x0000_d900_0353_0000, // 2 KiB — paper's MaskL at 8 KiB avg, level 2
    0x0000_d901_0353_0000, // 4 KiB
    0x0000_d903_0353_0000, // 8 KiB — paper's MaskA
    0x0000_d903_1353_0000, // 16 KiB
    0x0000_d90f_0353_0000, // 32 KiB — paper's MaskS at 8 KiB avg, level 2
    0x0000_d903_0353_7000, // 64 KiB
    0x0000_d907_0353_7000, // 128 KiB
    0x0000_d907_0753_7000, // 256 KiB
    0x0000_d917_0753_7000, // 512 KiB
    0x0000_d917_4753_7000, // 1 MiB
    0x0000_d917_6753_7000, // 2 MiB
    0x0000_d937_6753_7000, // 4 MiB
    0x0000_d937_7753_7000, // 8 MiB
    0x0000_d937_7757_7000, // 16 MiB
    0x0000_db37_7757_7000, // level +1 guard, only used at level 3 on 16 MiB
];

/// Rounded base-2 logarithm of `value`: the table index whose target chunk
/// size is closest to `value`.
///
/// Implemented in integer arithmetic (`f64::log2` is `std`-only, this crate
/// is `no_std`): with `f = floor(log2(v))`, rounding up happens exactly when
/// `v >= 1.5 * 2^f`, i.e. `v >= 3 << (f - 1)`. Ties cannot occur because
/// `1.5 * 2^f` is never an integer.
///
/// Callers guarantee `value` fits the table's supported span; see
/// [`crate::Params`].
pub(crate) fn logarithm2(value: usize) -> u32 {
    debug_assert!((64..=(1 << 24)).contains(&value));
    let floor = (usize::BITS - 1) - value.leading_zeros();
    if floor >= 1 && value >= (3usize << (floor - 1)) {
        floor + 1
    } else {
        floor
    }
}

/// Selects the `(mask_strict, mask_loose)` pair for `avg_size` at a
/// normalization level: `MASKS[bits + level]` below the average and
/// `MASKS[bits - level]` at or beyond it.
///
/// The strict mask has more one-bits, so a match is rarer and cuts are
/// delayed while the chunk is still smaller than average; the loose mask
/// has fewer one-bits, so cuts come sooner once the chunk has outgrown the
/// average. Both masks for level `l` keep `bits ± l` one-bits, which is the
/// paper's normalization level.
pub(crate) fn select_masks(avg_size: usize, level: Normalization) -> (u64, u64) {
    let bits = logarithm2(avg_size) as usize;
    let shift = level.bits() as usize;
    (MASKS[bits + shift], MASKS[bits - shift])
}

/// The normalization level of the chunk-size distribution, section 4.4 /
/// figure 9 of the 2016 paper.
///
/// Higher levels bias cut points harder toward the average size: level `l`
/// judges positions below the average with `bits + l` one-bits in the mask
/// and positions at or beyond it with `bits - l`. Level 0 disables
/// normalization and produces the wide, exponential chunk-size spread.
/// Level 2 is the paper's recommended setting and this crate's default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Normalization {
    /// No normalization: one mask for the whole scan, the widest spread.
    Level0,
    /// One extra bit of normalization: `(14, 12)` on an 8 KiB target.
    Level1,
    /// Two bits: `(15, 11)` on 8 KiB. The paper's sweet spot and default.
    #[default]
    Level2,
    /// Three bits: `(16, 10)` on 8 KiB, approaching fixed-size chunking.
    Level3,
}

impl Normalization {
    /// The mask-bit offset applied above and below the average size.
    pub(crate) const fn bits(self) -> u32 {
        match self {
            Normalization::Level0 => 0,
            Normalization::Level1 => 1,
            Normalization::Level2 => 2,
            Normalization::Level3 => 3,
        }
    }
}
