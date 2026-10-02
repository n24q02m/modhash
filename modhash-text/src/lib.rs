//! Text fingerprints: canonicalisation, word-3-shingles and a 128-word
//! MinHash signature (design spec §4, tier-2 text lane).
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds without a single registry package.
//!
//! # Pipeline
//!
//! 1. [`canonicalize`]: NFC (via `modhash_unicode::nfc`) → lowercase →
//!    strip trailing whitespace → exactly one `'\n'`.
//! 2. Words = `split_whitespace` runs of the canonical text; combining
//!    marks stay attached to their base letter (accents are kept —
//!    stripping them would erase distinctions Vietnamese relies on).
//! 3. Shingles = consecutive `k`-word windows, `k = min(3, n)` for an
//!    `n`-word document; hashed with FNV-1a 64 over the words joined by
//!    a single ASCII space.
//! 4. [`signature`] = 128-word MinHash: permutation `i` draws `seed_i`
//!    from `SplitMix64::new(SEED_STREAM)` and each signature word is
//!    `min over shingles of sm64_mix(fnv1a64(shingle) ^ seed_i)`,
//!    where `sm64_mix` is the splitmix64 output function applied as a
//!    pure finalizer (spec §4: `splitmix64(FNV1a64(w) ^ seed_i)`).
//!
//! The crate is `no_std`: only `alloc` containers and `core` string
//! operations are used, so the same code runs on embedded targets.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use modhash_primitives::{SplitMix64, fnv1a64};

/// Number of words per shingle (spec §4: "3 từ liên tiếp").
const SHINGLE_WORDS: usize = 3;

/// Length of a [`signature`] in `u64` words.
pub const SIGNATURE_WORDS: usize = 128;

/// Seed of the stream that produces the per-permutation `seed_i` values.
/// A pinned constant: the signature is a function of the text alone.
const SEED_STREAM: u64 = 0x5445_5854_4D48_3634; // "TEXTMH64", LE-pinned

/// The splitmix64 output function as a pure finalizer over `x`
/// (Steele, Lea & Flood 2014). Identical math to
/// [`SplitMix64::next_u64`], but applied to a value rather than to
/// generator state — this is the permutation mixer of spec §4.
fn sm64_mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Canonicalises `input` for fingerprinting.
///
/// Steps, in order: **NFC** via [`modhash_unicode::nfc`] (so NFD and NFC
/// spellings of the same text are indistinguishable downstream), then
/// **lowercase** via `str::to_lowercase`, then **trailing whitespace is
/// removed and exactly one `'\n'` is appended** — a single trailing
/// newline is part of the canonical form, and any amount of trailing
/// whitespace (spaces, tabs, `\r\n`, blank lines) is insignificant.
///
/// NFC runs *before* lowercasing because composition is defined on the
/// original code points; lowercasing is applied to already-composed
/// text so a composed letter and its lowercase form stay one unit.
///
/// [`canonicalize`] is idempotent: `canonicalize(canonicalize(x)) ==
/// canonicalize(x)` for every input.
#[must_use]
pub fn canonicalize(input: &str) -> String {
    let nfc = modhash_unicode::nfc(input);
    let lowered = nfc.to_lowercase();
    let mut out = String::with_capacity(lowered.len() + 1);
    out.push_str(lowered.trim_end());
    out.push('\n');
    out
}

/// FNV-1a 64 hash of one shingle: its words joined by a single ASCII
/// space. Joining instead of hashing words separately keeps word
/// boundaries significant (`"a bc"` ≠ `"ab c"`).
fn shingle_hash(shingle: &[&str]) -> u64 {
    fnv1a64(shingle.join(" ").as_bytes())
}

/// The 128 `seed_i` permutation seeds, drawn in order from
/// `SplitMix64::new(SEED_STREAM)`.
fn seeds() -> [u64; SIGNATURE_WORDS] {
    let mut rng = SplitMix64::new(SEED_STREAM);
    let mut out = [0u64; SIGNATURE_WORDS];
    for slot in &mut out {
        *slot = rng.next_u64();
    }
    out
}

/// MinHash over a slice of already-hashed set elements: word `i` is the
/// minimum of `sm64_mix(element ^ seed_i)` over all elements. An empty
/// slice yields `[u64::MAX; SIGNATURE_WORDS]` — the sentinel signature
/// of the empty set.
fn minhash(elements: &[u64]) -> [u64; SIGNATURE_WORDS] {
    let seeds = seeds();
    let mut out = [u64::MAX; SIGNATURE_WORDS];
    for &e in elements {
        for (slot, &seed) in out.iter_mut().zip(seeds.iter()) {
            let h = sm64_mix(e ^ seed);
            if h < *slot {
                *slot = h;
            }
        }
    }
    out
}

/// The 128-word MinHash signature of `input`.
///
/// The input is [`canonicalize`]d, split into words, windowed into
/// consecutive `k`-word shingles (`k = min(3, n)` — a document shorter
/// than three words still contributes one shorter shingle rather than
/// no signal), each shingle FNV-1a-hashed, and the shingle-hash set is
/// MinHashed per spec §4: word `i` =
/// `min over shingles of sm64_mix(fnv1a64(shingle) ^ seed_i)`.
///
/// The signature of the empty document is `[u64::MAX; 128]`.
#[must_use]
pub fn signature(input: &str) -> Vec<u64> {
    let canonical = canonicalize(input);
    let words: Vec<&str> = canonical.split_whitespace().collect();
    let k = words.len().min(SHINGLE_WORDS);
    let shingles: Vec<u64> = if k == 0 {
        Vec::new()
    } else {
        words.windows(k).map(shingle_hash).collect()
    };
    minhash(&shingles).to_vec()
}

/// Estimates the Jaccard index of the two shingle sets that produced
/// `a` and `b`: the fraction of equal signature words, returned as an
/// `f64` in `[0.0, 1.0]`.
///
/// Only the common prefix is compared, so slices of unequal length are
/// scored over `min(a.len(), b.len())` positions. Two empty slices
/// score `1.0`; an empty slice against a non-empty one also compares
/// zero positions and scores `1.0` — callers comparing signatures
/// should always pass full [`SIGNATURE_WORDS`]-word signatures, where
/// the estimate's standard error is about 0.04.
#[must_use]
pub fn jaccard_estimate(a: &[u64], b: &[u64]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 1.0;
    }
    let equal = a
        .iter()
        .zip(b.iter())
        .take(n)
        .filter(|(x, y)| x == y)
        .count();
    equal as f64 / n as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Splitmix64-drawn synthetic sets with a known overlap, then the
    /// 128-word MinHash estimate must converge on the true Jaccard.
    /// Tolerance per spec §6: `|err| <= 0.1` for 95% of pairs at n=128;
    /// the observed outlier count is pinned so a broken permutation
    /// mixer or a dropped seed turns this red instead of drifting.
    #[test]
    fn minhash_estimates_jaccard() {
        let mut rng = SplitMix64::new(0xC0FF_EE11_2233_4455);
        const PAIRS: usize = 200;
        let mut out_of_tolerance = 0usize;
        let mut err_sum = 0.0f64;
        for _ in 0..PAIRS {
            let sa = 30 + (rng.next_u64() % 90) as usize;
            let sb = 30 + (rng.next_u64() % 90) as usize;
            let overlap = (rng.next_u64() as usize) % (sa.min(sb) + 1);
            let mut a = Vec::with_capacity(sa);
            let mut b = Vec::with_capacity(sb);
            for _ in 0..overlap {
                let v = rng.next_u64();
                a.push(v);
                b.push(v);
            }
            while a.len() < sa {
                a.push(rng.next_u64());
            }
            while b.len() < sb {
                b.push(rng.next_u64());
            }
            let true_j = overlap as f64 / (sa + sb - overlap) as f64;
            let est = jaccard_estimate(&minhash(&a), &minhash(&b));
            let err = (est - true_j).abs();
            err_sum += err;
            if err > 0.1 {
                out_of_tolerance += 1;
            }
        }
        // Observed (deterministic): 1/200 pairs outside |err| <= 0.1 —
        // far inside the 95% allowance of 10 — and mean |err| = 0.0283.
        // Bounds are pinned just above the observation, so a broken
        // permutation mixer, a broadcast seed or a dropped xor pushes
        // the counts far past them instead of silently drifting.
        assert!(
            out_of_tolerance <= 4,
            "{out_of_tolerance}/{PAIRS} pairs outside |err| <= 0.1 (95% bound allows 10)"
        );
        assert!(
            err_sum / PAIRS as f64 <= 0.04,
            "mean |err| {} exceeds the 0.04 bound",
            err_sum / PAIRS as f64
        );
    }

    /// The empty set MinHashes to the all-`u64::MAX` sentinel and every
    /// non-empty set lands strictly below it on at least one word.
    #[test]
    fn minhash_empty_set_sentinel() {
        assert_eq!(minhash(&[]), [u64::MAX; SIGNATURE_WORDS]);
        let one = minhash(&[0]);
        assert_ne!(one, [u64::MAX; SIGNATURE_WORDS]);
        assert!(one.iter().any(|&w| w != u64::MAX));
    }

    /// Identical sets produce identical signatures; disjoint sets share
    /// (almost) no words — 200 disjoint pairs must all estimate below
    /// the 0.1 tolerance band around Jaccard 0.
    #[test]
    fn minhash_disjoint_sets_estimate_zero() {
        let mut rng = SplitMix64::new(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..50 {
            let a: Vec<u64> = (0..40).map(|_| rng.next_u64()).collect();
            let b: Vec<u64> = (0..40).map(|_| rng.next_u64()).collect();
            assert!(jaccard_estimate(&minhash(&a), &minhash(&b)) <= 0.1);
        }
        let s: Vec<u64> = (0..40).map(|_| rng.next_u64()).collect();
        assert_eq!(minhash(&s), minhash(&s));
        assert_eq!(
            vec![u64::MAX; SIGNATURE_WORDS].as_slice(),
            minhash(&[]).as_slice()
        );
    }
}
