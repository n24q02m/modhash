//! The 128-word MinHash over the frame-hash chain — the video tier-2
//! signature's set-valued half (spec §4.2: "MinHash over the frame-hash
//! sequence").
//!
//! The chain is shingled into consecutive runs of [`SHINGLE_FRAMES`]
//! frame hashes — the same width the text lane uses over words
//! (`modhash-text`, spec §4), so one shingle captures 1.5 s of temporal
//! order at the fixed 2 fps sample rate. A chain shorter than the width
//! contributes a single shorter shingle rather than no signal.
//! Each shingle is FNV-1a-hashed over the frame hashes' big-endian words
//! (BE so the word order — the *temporal* order — is what the hash sees),
//! and the shingle-hash set is MinHashed exactly like spec §4:
//! signature word `i` = `min over shingles of sm64_mix(hash ^ seed_i)`
//! with `seed_i` drawn from `SplitMix64::new(SEED_STREAM)`.
//!
//! `SEED_STREAM` differs from the text lane's on purpose: the two
//! signatures are never compared across modalities, and an independent
//! seed stream keeps the video lane from inheriting any accidental
//! correlation with text's permutation family.

use modhash_primitives::{SplitMix64, fnv1a64};
use modhash_text::SIGNATURE_WORDS;

/// Consecutive frame hashes per shingle — matches the text lane's
/// 3-word shingle width.
pub const SHINGLE_FRAMES: usize = 3;

/// Seed of the stream producing the per-permutation `seed_i` values.
/// `0x56_49_44_45_4d_48_36_34` is `"VIDEMH64"` little-endian — pinned,
/// so the signature is a function of the frame chain alone.
const SEED_STREAM: u64 = 0x5649_4445_4D48_3634;

/// The splitmix64 output function as a pure finalizer over `x`
/// (Steele, Lea & Flood 2014) — the permutation mixer of spec §4, the
/// same math `modhash-text` applies inside `min over shingles`.
fn sm64_mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The 128 permutation seeds, drawn in order from
/// `SplitMix64::new(SEED_STREAM)`.
fn seeds() -> [u64; SIGNATURE_WORDS] {
    let mut rng = SplitMix64::new(SEED_STREAM);
    let mut out = [0u64; SIGNATURE_WORDS];
    for slot in &mut out {
        *slot = rng.next_u64();
    }
    out
}

/// FNV-1a 64 of one shingle: the frame hashes' big-endian encodings
/// concatenated, so word order inside the shingle is significant.
fn shingle_hash(shingle: &[u64]) -> u64 {
    let mut buf = Vec::with_capacity(shingle.len() * 8);
    for h in shingle {
        buf.extend_from_slice(&h.to_be_bytes());
    }
    fnv1a64(&buf)
}

/// The 128-word MinHash signature of a frame-hash chain.
///
/// The chain is windowed into consecutive [`SHINGLE_FRAMES`]-wide
/// shingles (`k = min(3, len)` for a short chain — the text lane's rule,
/// so a two-frame clip still contributes one shingle). Word `i` of the
/// signature is `min over shingles of sm64_mix(shingle_hash ^ seed_i)`.
/// The signature of the empty chain is `[u64::MAX; 128]` — the sentinel
/// signature of the empty set, same as `modhash-text`.
#[must_use]
pub fn minhash(frame_hashes: &[u64]) -> Vec<u64> {
    let k = frame_hashes.len().min(SHINGLE_FRAMES);
    let shingles: Vec<u64> = if k == 0 {
        Vec::new()
    } else {
        frame_hashes.windows(k).map(shingle_hash).collect()
    };
    let seeds = seeds();
    let mut out = vec![u64::MAX; SIGNATURE_WORDS];
    for &e in &shingles {
        for (slot, &seed) in out.iter_mut().zip(seeds.iter()) {
            let h = sm64_mix(e ^ seed);
            if h < *slot {
                *slot = h;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_is_sentinel() {
        assert_eq!(minhash(&[]), vec![u64::MAX; SIGNATURE_WORDS]);
    }

    #[test]
    fn word_count_is_signature_words() {
        assert_eq!(minhash(&[1, 2, 3, 4, 5]).len(), SIGNATURE_WORDS);
    }

    #[test]
    fn shingle_width_is_short_for_short_chains() {
        // A 2-element chain uses k=2: minhash(&[a,b]) must equal
        // minhash over the single shingle [a,b] — and must differ from
        // a chain whose only shingle is [a] alone.
        let ab = minhash(&[0x1111, 0x2222]);
        let a = minhash(&[0x1111]);
        assert_ne!(ab, a);
    }

    #[test]
    fn order_is_significant() {
        // The same frame multiset in reverse order is a different
        // shingle set, so the signatures differ.
        let fwd = minhash(&[1, 2, 3, 4, 5, 6]);
        let rev = minhash(&[6, 5, 4, 3, 2, 1]);
        assert_ne!(fwd, rev);
    }
}
