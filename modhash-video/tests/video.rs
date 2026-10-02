//! Acceptance tests for `modhash-video` (spec §4.2, plan P14).
//!
//! Fixtures are locally synthesized mp4/mov files — ffmpeg 9.0.1 +
//! libx264, baseline profile, generated on this machine; see
//! `fixtures/PROVENANCE.md` for the exact commands and hashes. The
//! pipeline is demux → h264 decode → 2 fps sampling → frame pHash
//! chain → MinHash, so every test exercises the real path end to end.

use modhash_primitives::{Error, SplitMix64};
use modhash_video::{
    FRAME_HAMMING_MAX, Limits, MATCH_SCORE_MIN, VideoFingerprint, decode, match_score, video_match,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/{name}"))
        .unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

fn fp(name: &str) -> VideoFingerprint {
    decode(&fixture(name), &Limits::default())
        .unwrap_or_else(|e| panic!("{name} must fingerprint: {e}"))
}

// ---------------------------------------------------------------------
// Facts and determinism
// ---------------------------------------------------------------------

#[test]
fn decode_reports_facts() {
    let v = fp("a_64x48.mp4");
    assert_eq!((v.width, v.height), (64, 48));
    assert_eq!(v.duration, 4.0);
    // 4 s at 8 fps source → 8 half-second slots, one frame each.
    assert_eq!(v.frame_hashes.len(), 8);
    assert!((v.fps_sampled - 2.0).abs() < 1e-9, "{}", v.fps_sampled);
    assert_eq!(v.minhash.len(), modhash_text::SIGNATURE_WORDS);
}

#[test]
fn decode_is_deterministic() {
    let bytes = fixture("a_64x48.mp4");
    let a = decode(&bytes, &Limits::default()).unwrap();
    let b = decode(&bytes, &Limits::default()).unwrap();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------
// Ground-truth clustering: same content closes, different contents far
// ---------------------------------------------------------------------

/// The separation invariant of spec §4.2: every same-content pair
/// (re-encoded resolution/bitrate, remuxed container) outscores every
/// cross-content pair, with a gap the match bound sits inside.
#[test]
fn same_content_variants_close_cross_content_far() {
    let a = fp("a_64x48.mp4");
    let same = [
        ("a_128x96", fp("a_128x96.mp4")),
        ("a_crf40", fp("a_crf40.mp4")),
        ("a_mov", fp("a_64x48.mov")),
    ];
    let cross = [
        ("b_mandelbrot", fp("b_64x48.mp4")),
        ("c_rgbtestsrc", fp("c_64x48.mp4")),
        ("d_gradients", fp("d_64x48.mp4")),
    ];

    let mut min_same = f64::MAX;
    let mut max_cross = f64::MIN;
    for (name, v) in &same {
        let m = video_match(&a, v);
        assert!(
            m.score >= 0.875,
            "same-content pair a vs {name} scored {}",
            m.score
        );
        assert!(m.matched);
        min_same = min_same.min(m.score);
    }
    for (name, v) in &cross {
        let m = video_match(&a, v);
        assert!(
            m.score <= 0.125,
            "cross-content pair a vs {name} scored {}",
            m.score
        );
        assert!(!m.matched);
        max_cross = max_cross.max(m.score);
    }
    // No overlap: the weakest same pair strictly outscores the best
    // cross pair — the bound MATCH_SCORE_MIN sits in the gap.
    assert!(min_same > max_cross);
    assert!(max_cross < MATCH_SCORE_MIN && MATCH_SCORE_MIN <= min_same);
}

/// A remuxed container (mp4 → mov, same ISO-BMFF box tree) carries the
/// identical stream: identical frame hashes, MinHash and tier-1 digest.
#[test]
fn mov_remux_is_identical_fingerprint() {
    let mp4 = fp("a_64x48.mp4");
    let mov = fp("a_64x48.mov");
    assert_eq!(mp4.frame_hashes, mov.frame_hashes);
    assert_eq!(mp4.minhash, mov.minhash);
    assert_eq!(mp4.content_digest, mov.content_digest);
    assert_eq!(video_match(&mp4, &mov).score, 1.0);
}

/// Different bitrate of the same content: frame hashes may drift a few
/// bits (score still perfect at hamming≤10), but tier-1 — defined over
/// decoded pixels — must differ because the pixels do.
#[test]
fn reencode_keeps_signature_but_not_tier1() {
    let a = fp("a_64x48.mp4");
    let crf = fp("a_crf40.mp4");
    assert_eq!(video_match(&a, &crf).score, 1.0);
    assert_ne!(a.content_digest, crf.content_digest);
}

// ---------------------------------------------------------------------
// match() semantics: temporal order, index-aligned
// ---------------------------------------------------------------------

#[test]
fn match_is_temporal_order_not_multiset() {
    // d_64x48's sampled chain is heterogeneous enough that reversing
    // it destroys index alignment entirely — observed 0.000.
    let v = fp("d_64x48.mp4");
    let mut reversed = v.frame_hashes.clone();
    reversed.reverse();
    let s = match_score(&v.frame_hashes, &reversed);
    assert!(s <= 0.125, "reversed chain scored {s}");

    // Index alignment: [x, A] vs [A, x] pairs x↔A and A↔x.
    let (x, y) = (0xFFFF_FFFF_FFFF_FFFFu64, 0u64);
    assert_eq!(match_score(&[x, y], &[y, x]), 0.0);
    assert_eq!(match_score(&[x, y], &[x, y]), 1.0);
    // Unequal lengths compare the common prefix only.
    assert_eq!(match_score(&[x, x], &[x]), 1.0);
    // Empty-vs-empty is the jaccard_estimate convention: identical.
    assert_eq!(match_score(&[], &[]), 1.0);
}

#[test]
fn match_hamming_bound_is_exact() {
    // Frame match predicate: hamming(a,b) <= FRAME_HAMMING_MAX. One
    // chain identical except for a single frame at the boundary on
    // each side must score exactly as the predicate says.
    let base = 0xAAAA_AAAA_AAAA_AAAAu64;
    let at_bound = base ^ ((1u64 << FRAME_HAMMING_MAX) - 1); // 10 bits
    let past_bound = base ^ ((1u64 << (FRAME_HAMMING_MAX + 1)) - 1); // 11 bits
    assert_eq!(match_score(&[base], &[at_bound]), 1.0);
    assert_eq!(match_score(&[base], &[past_bound]), 0.0);
}

// ---------------------------------------------------------------------
// Refusals: named errors, never panics
// ---------------------------------------------------------------------

#[test]
fn unsupported_and_malformed_are_named() {
    // Fragmented mp4: the demuxer's Unsupported reaches the caller.
    assert!(matches!(
        decode(&fixture("a_frag.mp4"), &Limits::default()),
        Err(Error::Unsupported(_))
    ));
    // Non-AVC codec (mpeg4 part 2 in the video track's stsd).
    assert!(matches!(
        decode(&fixture("e_mp4v.mp4"), &Limits::default()),
        Err(Error::Unsupported(_))
    ));
    // High-profile h264 refuses inside the decoder by profile name.
    match decode(&fixture("high_64x48.mp4"), &Limits::default()) {
        Err(Error::Unsupported(m)) => assert!(m.contains("High"), "{m}"),
        other => panic!("high profile must be Unsupported, got {other:?}"),
    }
    // An mp4 with only an audio track.
    assert!(matches!(
        decode(&fixture("audioonly.mp4"), &Limits::default()),
        Err(Error::BadValue(_))
    ));
    // Garbage and emptiness: magic-layer errors, no panic.
    assert!(matches!(
        decode(&[], &Limits::default()),
        Err(Error::InvalidMagic { .. }) | Err(Error::Truncated { .. })
    ));
    assert!(matches!(
        decode(&[0xAB; 4096], &Limits::default()),
        Err(Error::InvalidMagic { .. }) | Err(Error::Truncated { .. })
    ));
}

#[test]
fn limits_are_enforced() {
    let bytes = fixture("a_64x48.mp4");
    let v = decode(&bytes, &Limits::default()).unwrap();

    let lim = Limits {
        max_input: bytes.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(decode(&bytes, &lim), Err(Error::TooLarge { .. })));

    let lim = Limits {
        max_duration_s: v.duration - 0.5,
        ..Limits::default()
    };
    assert!(matches!(decode(&bytes, &lim), Err(Error::TooLarge { .. })));

    let lim = Limits {
        max_frames: v.frame_hashes.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(decode(&bytes, &lim), Err(Error::TooLarge { .. })));

    // Fewer decodable frames than the stream contains → TooLarge.
    let lim = Limits {
        max_decoded_frames: 2,
        ..Limits::default()
    };
    assert!(matches!(decode(&bytes, &lim), Err(Error::TooLarge { .. })));
}

/// 20 000 deterministic corruptions of a valid mp4 — bit flips, drops,
/// splices, inserts — must never panic; `Err` is the correct answer
/// for nearly all of them.
#[test]
fn mutated_mp4_never_panics() {
    let file = fixture("tiny_16x16.mp4");
    let mut rng = SplitMix64::new(0x0B17_F11F);
    let mut ok = 0usize;
    for _ in 0..20_000 {
        let mut bad = file.clone();
        match rng.next_u64() % 4 {
            0 => {
                for _ in 0..1 + rng.next_u64() % 8 {
                    let i = (rng.next_u64() as usize) % bad.len();
                    bad[i] ^= 1 << (rng.next_u64() % 8);
                }
            }
            1 => bad.truncate((rng.next_u64() as usize) % (bad.len() + 1)),
            2 => {
                if !bad.is_empty() {
                    let a = (rng.next_u64() as usize) % bad.len();
                    let b = (rng.next_u64() as usize) % bad.len();
                    let (lo, hi) = (a.min(b), a.max(b));
                    bad.drain(lo..hi.max(lo + 1).min(bad.len()));
                }
            }
            _ => {
                let at = (rng.next_u64() as usize) % (bad.len() + 1);
                let v = rng.next_u64() as u8;
                for _ in 0..rng.next_u64() % 16 {
                    bad.insert(at.min(bad.len()), v);
                }
            }
        }
        let r = std::panic::catch_unwind(|| decode(&bad, &Limits::default()));
        assert!(r.is_ok(), "decode panicked on mutated input");
        ok += usize::from(r.unwrap().is_ok());
    }
    // Sanity: mutations actually explore — some corruptions still
    // decode (flips inside mdat payload bytes), most do not. Both
    // directions observed proves the loop isn't vacuous.
    assert!(ok < 20_000);
}

/// Every strict prefix of a valid mp4: no panic, and none may
/// fingerprint (a prefix of a required table can't be complete).
#[test]
fn all_prefixes_no_panic() {
    let file = fixture("tiny_16x16.mp4");
    for n in 0..file.len() {
        let r = std::panic::catch_unwind(|| decode(&file[..n], &Limits::default()));
        assert!(r.is_ok(), "decode panicked on prefix len {n}");
        assert!(r.unwrap().is_err(), "prefix len {n} fingerprinted");
    }
    // Full file is good — the loop is over a real fixture.
    assert!(decode(&file, &Limits::default()).is_ok());
}
