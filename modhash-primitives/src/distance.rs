//! The one distance metric the kit specifies: Hamming distance between
//! digests.

use crate::digest::Digest;

/// Widens a byte chunk to `u64`, first byte most significant. A chunk
/// shorter than eight bytes is zero-padded on the right; when both
/// sides are widened, as [`hamming`] does, the padding is identical and
/// cancels out in the XOR.
fn widen(chunk: &[u8]) -> u64 {
    chunk.iter().fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte))
}

/// The number of differing bits between two digests.
///
/// This is the single comparison used by every perceptual crate and the
/// index layer, so it is specified once, here. It walks eight bytes at
/// a time and counts with `u64::count_ones` — one `POPCNT`-class
/// instruction per stride, never a per-bit loop; the tail of fewer than
/// eight bytes is widened and counted the same way.
///
/// ```
/// let a = modhash_primitives::Digest::<1>::from_bytes([0b0000_1111]);
/// let b = modhash_primitives::Digest::<1>::from_bytes([0b0000_0001]);
/// assert_eq!(modhash_primitives::hamming(&a, &b), 3);
/// ```
pub fn hamming<const N: usize>(a: &Digest<N>, b: &Digest<N>) -> u32 {
    let mut total = 0u32;
    let mut a_chunks = a.as_bytes().chunks_exact(8);
    let mut b_chunks = b.as_bytes().chunks_exact(8);
    for (x, y) in a_chunks.by_ref().zip(b_chunks.by_ref()) {
        total += (widen(x) ^ widen(y)).count_ones();
    }
    total += (widen(a_chunks.remainder()) ^ widen(b_chunks.remainder())).count_ones();
    total
}
