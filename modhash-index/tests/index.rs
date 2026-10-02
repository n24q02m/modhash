//! Conformance tests for `modhash-index`: the BK-tree against a linear
//! scan, the LSH index against its banding contract, and `calibrate`
//! against hand-computed optima.
//!
//! All randomness is `modhash_primitives::SplitMix64` with pinned seeds,
//! so every expected set and every measured count reproduces bit-exactly.

use modhash_index::{BkTree, LshIndex, Profile, calibrate};
use modhash_primitives::{Error, SplitMix64};

/// Hamming distance on raw u64 hashes — the same metric the tree uses,
/// computed independently of the tree for the reference scan.
fn hamming64(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// The `O(n)` reference: every entry within `radius` of `query`, as a
/// sorted `(distance, id)` vector — the BK-tree must reproduce it
/// exactly.
fn brute_force(entries: &[(u64, u64)], query: u64, radius: u32) -> Vec<(u32, u64)> {
    let mut out: Vec<(u32, u64)> = entries
        .iter()
        .filter_map(|&(hash, id)| {
            let d = hamming64(query, hash);
            (d <= radius).then_some((d, id))
        })
        .collect();
    out.sort_unstable();
    out
}

// ---------------------------------------------------------------- BK-tree

/// The P17 reference test: 1000 deterministic hashes, five query seeds,
/// radii {0, 1, 3, 8, 16} — the tree's answer must equal the linear
/// scan's answer at every point.
#[test]
fn bk_tree_matches_brute_force() {
    let mut rng = SplitMix64::new(0x00B1_7EE5);
    let entries: Vec<(u64, u64)> = (0..1000u64).map(|i| (rng.next_u64(), i)).collect();
    let mut tree = BkTree::new();
    for &(hash, id) in &entries {
        tree.insert(hash, id);
    }
    assert_eq!(tree.len(), 1000);

    for q in 0..5u64 {
        let mut qrng = SplitMix64::new(0x5EED ^ q);
        let query = qrng.next_u64();
        for radius in [0u32, 1, 3, 8, 16] {
            let got: Vec<(u32, u64)> = tree
                .search(query, radius)
                .iter()
                .map(|c| (c.distance, c.id))
                .collect();
            let want = brute_force(&entries, query, radius);
            assert_eq!(
                got, want,
                "bk-tree diverged from the linear scan at radius {radius} (query {query:#018x})"
            );
        }
    }
}

/// Queries are not only unseen points: searching for a hash that was
/// inserted must return at least that entry at distance 0.
#[test]
fn bk_tree_finds_inserted_members() {
    let mut rng = SplitMix64::new(7);
    let mut tree = BkTree::new();
    for i in 0..500u64 {
        tree.insert(rng.next_u64(), i);
    }
    let mut probe = SplitMix64::new(7); // same seed → same hash stream
    for i in 0..500u64 {
        let hash = probe.next_u64();
        let found = tree.search(hash, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, i);
        assert_eq!(found[0].distance, 0);
    }
}

/// Equal hashes chain down distance-0 edges; all of them must come back
/// from a radius-0 search, and nothing else may.
#[test]
fn bk_tree_handles_duplicate_hashes_and_ids() {
    let mut tree = BkTree::new();
    tree.insert(0xDEAD_BEEF, 1);
    tree.insert(0xDEAD_BEEF, 2);
    tree.insert(0xDEAD_BEEF, 3);
    tree.insert(0xDEAD_BEEF, 1); // duplicate id too
    tree.insert(0x1234_5678, 9);

    let found = tree.search(0xDEAD_BEEF, 0);
    let ids: Vec<u64> = found.iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![1, 1, 2, 3]);
    assert!(found.iter().all(|c| c.distance == 0));
    assert_eq!(tree.len(), 5);
}

/// Empty and degenerate trees behave: no panic, no phantom results.
#[test]
fn bk_tree_edge_cases() {
    let mut tree = BkTree::new();
    assert!(tree.is_empty());
    assert!(tree.search(0, u32::MAX).is_empty());

    tree.insert(42, 99);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.search(42, 0).len(), 1);
    assert_eq!(tree.search(42, 64).len(), 1);
    assert!(tree.search(!42, 0).is_empty()); // distance 64 > radius 0
}

/// Worst-case geometry for the prune bound: a chain of hashes each 64
/// bits from its parent. The iterative walk must return the correct set
/// without recursing — this is the attacker-sized input the spec calls
/// out.
#[test]
fn bk_tree_deep_chain_stays_iterative() {
    let mut tree = BkTree::new();
    let n = 50_000u64;
    for i in 0..n {
        tree.insert(if i % 2 == 0 { 0 } else { u64::MAX }, i);
    }
    // All zeros are distance 0 from query 0, all MAXes are 64.
    assert_eq!(tree.search(0, 0).len() as u64, n / 2);
    assert_eq!(tree.search(u64::MAX, 0).len() as u64, n / 2);
    assert_eq!(tree.search(0, 64).len() as u64, n);
}

/// Result order is part of the contract: (distance, id) ascending,
/// reproducibly — required so downstream dedup/merge code sees stable
/// input.
#[test]
fn bk_tree_results_are_sorted() {
    let mut rng = SplitMix64::new(0xA11CE);
    let mut tree = BkTree::new();
    let hub = rng.next_u64();
    for i in 0..64u64 {
        // hash at distance d = i%8 from hub, id = i
        tree.insert(hub ^ ((1u64 << (i % 8)) - 1), i);
    }
    let found = tree.search(hub, 7);
    assert_eq!(found.len(), 64);
    let mut sorted = found.clone();
    sorted.sort_unstable();
    assert_eq!(found, sorted);
}

// ------------------------------------------------------------------- LSH

/// Builds a deterministic 128-word MinHash signature from a seed.
fn signature(seed: u64) -> Vec<u64> {
    let mut rng = SplitMix64::new(seed);
    (0..128).map(|_| rng.next_u64()).collect()
}

/// An exact copy is a candidate in every band.
#[test]
fn lsh_identical_signature_is_candidate() {
    let sig = signature(1);
    let mut index = LshIndex::new();
    index.insert(7, &sig).unwrap();
    assert_eq!(index.candidates(&sig).unwrap(), vec![7]);
}

/// The deterministic guarantee: a signature differing in `k < bands`
/// bit positions shares at least one whole band with the original, so
/// it must collide — no probability involved. This is the pinned
/// false-negative bound for near-duplicates closer than one stripe.
#[test]
fn lsh_near_duplicate_below_band_count_collides() {
    for k in 1u64..16 {
        let mut near = signature(2);
        // Flip k bits in distinct rows: each bit flip dirties its row's
        // band, so k flips can touch at most k of the 16 bands.
        for b in 0..k {
            let row = ((b * 37) % 128) as usize; // spread across bands
            near[row] ^= 1 << (b % 63);
        }
        let mut index = LshIndex::new();
        index.insert(1, &signature(2)).unwrap();
        index.insert(2, &near).unwrap();
        let cand = index.candidates(&near).unwrap();
        assert!(
            cand.contains(&1),
            "k={k} bit flips must leave >=1 shared band; candidates: {cand:?}"
        );
    }
}

/// A signature sharing no stripe yields no candidate — the index must
/// not over-report either.
#[test]
fn lsh_far_pair_yields_no_candidates() {
    let a = signature(3);
    let b = signature(4);
    let mut index = LshIndex::new();
    index.insert(1, &a).unwrap();
    index.insert(2, &b).unwrap();
    assert!(index.candidates(&a).unwrap() == vec![1]);
    assert!(index.candidates(&b).unwrap() == vec![2]);
}

/// Measured recall on a deterministic corpus of 200 near-pairs: each
/// pair shares a base signature and then differs in a seeded amount of
/// rows. The exact count is pinned — the plan asks for a *measured*
/// recall recorded in the test, and pinning the number is stronger
/// than pinning a bound.
#[test]
fn lsh_measured_recall_is_pinned() {
    let mut index = LshIndex::new();
    let mut probes = Vec::new();
    let mut rng = SplitMix64::new(0xC0FFEE);
    for i in 0..200u64 {
        let base = signature(rng.next_u64());
        let mut near = base.clone();
        // dirty 16..=48 distinct rows — at or beyond band count, so
        // collisions are the probabilistic regime worth measuring
        let dirty = 16 + (rng.next_u64() % 33) as usize;
        for _ in 0..dirty {
            let row = (rng.next_u64() % 128) as usize;
            near[row] ^= rng.next_u64() | 1; // nonzero change
        }
        index.insert(i, &base).unwrap();
        probes.push((i, near));
    }
    let mut hits = 0usize;
    for (id, near) in &probes {
        if index.candidates(near).unwrap().contains(id) {
            hits += 1;
        }
    }
    // Pinned measurement: revisit before changing the bucket mixer.
    assert_eq!(hits, 177, "LSH recall regression: expected 177/200");
}

/// Wrong-length signatures are rejected, not silently bucketed.
#[test]
fn lsh_rejects_wrong_signature_length() {
    let mut index = LshIndex::new();
    let short = vec![0u64; 127];
    let long = vec![0u64; 129];
    assert_eq!(
        index.insert(1, &short),
        Err(Error::BadValue("minhash signature length"))
    );
    assert_eq!(
        index.candidates(&long),
        Err(Error::BadValue("minhash signature length"))
    );
}

/// A caller-chosen geometry works end to end and is honored exactly.
#[test]
fn lsh_custom_shape() {
    let mut index = LshIndex::with_shape(8, 4);
    assert_eq!(index.signature_len(), 32);
    let mut rng = SplitMix64::new(9);
    let sig: Vec<u64> = (0..32).map(|_| rng.next_u64()).collect();
    index.insert(5, &sig).unwrap();
    index.insert(6, &sig).unwrap(); // second id, same signature
    let mut sorted = index.candidates(&sig).unwrap();
    sorted.sort_unstable();
    assert_eq!(sorted, vec![5, 6]);
}

/// Inserting the same id twice under the same signature does not
/// duplicate it in the candidate list (the output is a set).
#[test]
fn lsh_deduplicates_candidates() {
    let sig = signature(5);
    let mut index = LshIndex::new();
    index.insert(7, &sig).unwrap();
    index.insert(7, &sig).unwrap();
    assert_eq!(index.candidates(&sig).unwrap(), vec![7]);
}

// ------------------------------------------------------------- calibrate

/// A hand-computed set with known per-profile optima.
///
/// Positives {3, 5, 7, 9, 11}, negatives {4, 10, 20, 30, 40}; a
/// negative counts as fp whenever its distance is <= the threshold:
///
/// | t  | tp fp |   P   |   R   |  F0.5  |   F1   |   F2   |
/// | 3  |  1  0 | 1.000 | 0.200 | 0.5556 | 0.3333 | 0.2381 |
/// | 4  |  1  1 | 0.500 | 0.200 | 0.3846 | 0.2857 | 0.2273 |
/// | 9  |  4  1 | 0.800 | 0.800 | 0.8000 | 0.8000 | 0.8000 |
/// | 10 |  4  2 | 0.667 | 0.800 | 0.6897 | 0.7273 | 0.7692 |
/// | 11 |  5  2 | 0.714 | 1.000 | 0.7576 | 0.8333 | 0.9259 |
/// | 20 |  5  3 | 0.625 | 1.000 | 0.6757 | 0.7692 | 0.8929 |
/// | 40 |  5  5 | 0.500 | 1.000 | 0.5556 | 0.6667 | 0.8333 |
///
/// F0.5 peaks at t=9 (0.800): Dedup refuses the second negative that
/// t=11 swallows. F2 peaks at t=11 (0.9259): Search pays a false
/// positive for the last positive. One set, both optima.
#[test]
fn calibrate_finds_known_optimum() {
    let pairs: Vec<(u32, bool)> = [
        (3u32, true),
        (5, true),
        (7, true),
        (9, true),
        (11, true),
        (4, false),
        (10, false),
        (20, false),
        (30, false),
        (40, false),
    ]
    .into();
    let dedup = calibrate(&pairs, Profile::Dedup).unwrap();
    assert_eq!(dedup.threshold, Some(9));
    assert_eq!(dedup.true_positives, 4);
    assert_eq!(dedup.false_positives, 1);
    assert_eq!(dedup.false_negatives, 1);
    assert_eq!(dedup.true_negatives, 4);
    assert!((dedup.score - 0.8).abs() < 1e-12);

    let search = calibrate(&pairs, Profile::Search).unwrap();
    assert_eq!(search.threshold, Some(11));
    assert_eq!(search.true_positives, 5);
    assert_eq!(search.false_positives, 2);
    assert_eq!(search.false_negatives, 0);
    assert_eq!(search.true_negatives, 3);
    assert!((search.recall() - 1.0).abs() < 1e-12);
}

/// The two profiles must pick different thresholds on data where
/// precision and recall pull apart.
///
/// Positives {2, 4, 6, 8, 20}, negatives {3, 9, 10, 11, 30}:
///
/// | t  | tp fp |   P   |   R   |  F0.5  |   F1   |   F2   |
/// | 8  |  4  1 | 0.800 | 0.800 | 0.8000 | 0.8000 | 0.8000 |
/// | 9  |  4  2 | 0.667 | 0.800 | 0.6897 | 0.7273 | 0.7692 |
/// | 11 |  4  4 | 0.500 | 0.800 | 0.5405 | 0.6154 | 0.7143 |
/// | 20 |  5  4 | 0.556 | 1.000 | 0.6098 | 0.7143 | 0.8621 |
/// | 30 |  5  5 | 0.500 | 1.000 | 0.5556 | 0.6667 | 0.8333 |
///
/// F0.5 max: t=8 (0.8000). F2 max: t=20 (0.8621). Divergence proven:
/// Dedup stops at 8 while Search reaches past the negative cluster for
/// the last positive.
#[test]
fn calibrate_profiles_diverge() {
    let pairs: Vec<(u32, bool)> = [
        (2u32, true),
        (4, true),
        (6, true),
        (8, true),
        (20, true),
        (3, false),
        (9, false),
        (10, false),
        (11, false),
        (30, false),
    ]
    .into();
    let dedup = calibrate(&pairs, Profile::Dedup).unwrap();
    let search = calibrate(&pairs, Profile::Search).unwrap();
    assert_eq!(
        dedup.threshold,
        Some(8),
        "dedup must stop before the negatives"
    );
    assert_eq!(
        search.threshold,
        Some(20),
        "search must reach the last positive"
    );
    assert_eq!(dedup.false_positives, 1);
    assert_eq!(search.recall(), 1.0);
}

/// The plan's reproducibility clause: two calls on the same data must
/// return byte-identical answers, including the score.
#[test]
fn calibrate_is_reproducible() {
    let mut rng = SplitMix64::new(0xCA11B);
    let mut pairs = Vec::new();
    for _ in 0..400 {
        // positives cluster low, negatives cluster high — a realistic
        // overlapping calibration set
        let d = (rng.next_u64() % 24) as u32;
        pairs.push((d, d < 12 && rng.next_u64() % 4 != 0));
        let d = 10 + (rng.next_u64() % 30) as u32;
        pairs.push((d, d < 14 && rng.next_u64() % 5 == 0));
    }
    for profile in [Profile::Dedup, Profile::Search] {
        let a = calibrate(&pairs, profile).unwrap();
        let b = calibrate(&pairs, profile).unwrap();
        assert_eq!(a, b);
        // and the answer must be a real measured boundary
        assert!(a.threshold.is_some());
    }
}

/// A set with no positive pairs: the only sane verdict is "match
/// nothing" — anything else is an invented false positive.
#[test]
fn calibrate_no_positives_says_match_nothing() {
    let pairs = vec![(1u32, false), (5, false), (20, false)];
    for profile in [Profile::Dedup, Profile::Search] {
        let c = calibrate(&pairs, profile).unwrap();
        assert_eq!(c.threshold, None);
        assert_eq!(c.true_positives, 0);
        assert_eq!(c.false_positives, 0);
    }
}

/// An all-positive set accepts everything: the threshold is the largest
/// observed distance.
#[test]
fn calibrate_all_positives_accepts_all() {
    let pairs = vec![(3u32, true), (8, true), (8, true), (15, true)];
    let c = calibrate(&pairs, Profile::Search).unwrap();
    assert_eq!(c.threshold, Some(15));
    assert_eq!(c.true_positives, 4);
    assert_eq!(c.false_negatives, 0);
}

/// Empty input is a caller bug, reported, never a made-up threshold.
#[test]
fn calibrate_rejects_empty_input() {
    assert_eq!(
        calibrate(&[], Profile::Dedup),
        Err(Error::BadValue("calibration pairs"))
    );
    assert_eq!(
        calibrate(&[], Profile::Search),
        Err(Error::BadValue("calibration pairs"))
    );
}

/// Mixed labels at the same distance collapse to one boundary: with
/// `{10:true, 10:false, 10:false}` the only candidate threshold is 10,
/// both profiles return it, and the reported counts are honest —
/// precision 1/3, recall 1.0.
#[test]
fn calibrate_mixed_labels_at_one_distance() {
    let pairs = vec![(10u32, true), (10, false), (10, false)];
    for profile in [Profile::Dedup, Profile::Search] {
        let c = calibrate(&pairs, profile).unwrap();
        assert_eq!(c.threshold, Some(10));
        assert_eq!(c.true_positives, 1);
        assert_eq!(c.false_positives, 2);
        assert!((c.precision() - 1.0 / 3.0).abs() < 1e-12);
        assert!((c.recall() - 1.0).abs() < 1e-12);
    }
}

/// Duplicate distances, out-of-order input and unsorted candidates all
/// reduce to the same boundaries — shuffling the input must not change
/// the answer.
#[test]
fn calibrate_ignores_input_order() {
    let ordered: Vec<(u32, bool)> = [
        (1u32, true),
        (2, true),
        (3, true),
        (3, true),
        (4, false),
        (8, false),
        (8, false),
        (20, false),
    ]
    .into();
    let mut shuffled = ordered.clone();
    shuffled.reverse();
    shuffled.swap(1, 5);
    for profile in [Profile::Dedup, Profile::Search] {
        assert_eq!(
            calibrate(&ordered, profile).unwrap(),
            calibrate(&shuffled, profile).unwrap()
        );
    }
}
