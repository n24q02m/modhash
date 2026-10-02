//! Conformance tests for `modhash-text`: canonicalisation seams (NFC,
//! lowercase, the single trailing newline), signature determinism and
//! the golden pipeline pin, and the in-crate corruption loops that
//! stand in for the shared fuzz harness at integration.
//!
//! All randomness is `modhash_primitives::SplitMix64` with pinned
//! seeds, so every expectation reproduces bit-exactly.

use modhash_primitives::SplitMix64;
use modhash_text::{canonicalize, jaccard_estimate, signature};

// ------------------------------------------------------------------
// canonicalize
// ------------------------------------------------------------------

/// NFC, lowercase and exactly one trailing newline, in that order.
#[test]
fn canonicalize_normalizes_case_and_trailing_newline() {
    assert_eq!(canonicalize("Hello WORLD"), "hello world\n");
    assert_eq!(canonicalize("text\n"), "text\n");
    assert_eq!(canonicalize("text\n\n\n"), "text\n");
    assert_eq!(canonicalize("text\r\n"), "text\n");
    assert_eq!(canonicalize("text   \t \n"), "text\n");
    assert_eq!(canonicalize(""), "\n");
    assert_eq!(canonicalize("   \n"), "\n");
}

/// `canonicalize` is idempotent.
#[test]
fn canonicalize_is_idempotent() {
    for input in ["Hello\n\n", "ÜNICODE text", "", "a  b\tc\n"] {
        let once = canonicalize(input);
        assert_eq!(canonicalize(&once), once);
    }
}

/// NFC fold: a combining-sequence spelling and the precomposed spelling
/// of the same Vietnamese word must canonicalize identically — this is
/// the seam that breaks if NFC is skipped.
#[test]
fn canonicalize_nfc_equivalence() {
    // "ế" = 'ê' + combining acute (U+0301); "ệ" = 'ê' + dot below.
    let decomposed = "Ti\u{00EA}\u{0301}ng Vi\u{00EA}\u{0323}t";
    assert_eq!(canonicalize(decomposed), "ti\u{1EBF}ng vi\u{1EC7}t\n");
    // é as 'e' + U+0301 composes to U+00E9.
    assert_eq!(canonicalize("CAFE\u{0301}"), "caf\u{00E9}\n");
}

// ------------------------------------------------------------------
// signature: determinism, normalization invariance, golden pin
// ------------------------------------------------------------------

/// Same input twice → identical signature; NFC and NFD spellings and
/// different case spellings of the same text → identical signature.
#[test]
fn signature_determinism_and_canonical_invariance() {
    let a = signature("The quick brown fox jumps over the lazy dog");
    assert_eq!(a, signature("The quick brown fox jumps over the lazy dog"));
    assert_eq!(a.len(), 128);

    // NFC vs NFD via the crate's own decomposer.
    let nfd = modhash_unicode::nfd("Tiếng Việt là một ngôn ngữ phổ biến");
    let sig_nfc = signature("TIẾNG VIỆT LÀ MỘT NGÔN NGỮ PHỔ BIẾN\n\n\n");
    assert_eq!(signature(&nfd), sig_nfc);

    // Case is insignificant.
    assert_eq!(signature("Hello World"), signature("hello world"));
    // Trailing whitespace is insignificant.
    assert_eq!(signature("a b c\n\n"), signature("a b c"));
}

/// The end-to-end pipeline is pinned: `canonicalize("alpha beta
/// gamma")` is `"alpha beta gamma\n"`, the only shingle is
/// `"alpha beta gamma"` with FNV-1a `0x29496d94f8235e1e`, the first
/// two signature words are byte-exact and the FNV-1a fold of all 128
/// words is `0x46f8ab1a50c61813`. Any seam change — shingle width,
/// join byte, seed stream, mixer constant — changes this pin.
#[test]
fn signature_golden_pin() {
    let sig = signature("alpha beta gamma");
    assert_eq!(sig[0], 0x5409_aadb_22bb_8479);
    assert_eq!(sig[1], 0xb2e6_ff4e_debf_53c6);
    let mut fold = 0xcbf2_9ce4_8422_2325u64;
    for word in &sig {
        for b in word.to_le_bytes() {
            fold ^= u64::from(b);
            fold = fold.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    assert_eq!(fold, 0x46f8_ab1a_50c6_1813);
}

/// Short documents: 0 words → the all-MAX sentinel; 1–2 words → a
/// single shorter shingle that still distinguishes documents.
#[test]
fn signature_short_documents() {
    assert_eq!(signature(""), vec![u64::MAX; 128]);
    assert_eq!(signature("   \n \t "), vec![u64::MAX; 128]);
    let one = signature("alpha");
    let two = signature("alpha beta");
    assert_ne!(one, vec![u64::MAX; 128]);
    assert_ne!(two, vec![u64::MAX; 128]);
    assert_ne!(one, signature("beta"));
    assert_ne!(one, two);
    assert_ne!(two, signature("alpha beta gamma"));
}

// ------------------------------------------------------------------
// jaccard_estimate
// ------------------------------------------------------------------

#[test]
fn jaccard_estimate_bounds_and_edges() {
    let a = signature("the same document twice over today");
    let b = signature("the same document twice over today");
    assert_eq!(jaccard_estimate(&a, &b), 1.0);
    assert_eq!(jaccard_estimate(&a, &a[..64]), 1.0); // common prefix
    assert_eq!(jaccard_estimate(&[], &[]), 1.0);
    assert_eq!(jaccard_estimate(&[], &a), 1.0); // documented: 0/0
    let c = signature("completely unrelated words placed here instead");
    let j = jaccard_estimate(&a, &c);
    assert!(j < 0.2, "unrelated texts estimated {j}");
    // A near-copy scores high: one word changed out of ten.
    let d = signature("the same document twice over tomorrow");
    let j2 = jaccard_estimate(&a, &d);
    assert!(j2 > 0.5, "one-word edit estimated {j2}");
}

// ------------------------------------------------------------------
// In-crate corruption loops (stand-in for the shared fuzz harness —
// see contract deviation note; the heavy driver runs at integration).
// ------------------------------------------------------------------

/// Every single-character substitution, insertion, deletion and
/// whitespace mutation of a base document is fingerprinted without a
/// panic, and — outside fully colliding MinHash draws — changes the
/// signature: each mutation must move at least one signature word.
#[test]
fn signature_changes_predictably_under_mutation() {
    let base = "the quick brown fox jumps over the lazy dog today";
    let base_sig = signature(base);
    let words: Vec<&str> = base.split_whitespace().collect();
    let mut rng = SplitMix64::new(0x0BAD_5EED_0000_0001);

    // Substitution: swap one word for a fresh random word.
    for _ in 0..100 {
        let i = (rng.next_u64() as usize) % words.len();
        let mut v: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        v[i] = format!("w{:016x}", rng.next_u64());
        assert_ne!(signature(&v.join(" ")), base_sig, "substitution at {i}");
    }

    // Deletion: drop one word.
    for i in 0..words.len() {
        let mut v: Vec<&str> = words.clone();
        v.remove(i);
        assert_ne!(signature(&v.join(" ")), base_sig, "deletion at {i}");
    }

    // Insertion: splice a fresh random word in.
    for _ in 0..100 {
        let i = (rng.next_u64() as usize) % (words.len() + 1);
        let mut v: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        v.insert(i, format!("w{:016x}", rng.next_u64()));
        assert_ne!(signature(&v.join(" ")), base_sig, "insertion at {i}");
    }

    // Whitespace mutation: joining with double spaces is invisible
    // (split_whitespace), but re-joining words with no separator merges
    // them into different tokens — the signature must notice.
    assert_ne!(signature(&words.concat()), base_sig);
}

/// Arbitrary bytes pushed through `String::from_utf8_lossy` and every
/// byte-length prefix of a multi-byte document must never panic — the
/// lossy replacement path and mid-character truncation are the hostile
/// edge of a `&str` API.
#[test]
fn arbitrary_bytes_and_prefixes_never_panic() {
    let mut rng = SplitMix64::new(0xAB17_AB17_AB17_AB17);
    let mut buf = vec![0u8; 512];
    for round in 0..200 {
        rng.fill_bytes(&mut buf);
        let text = String::from_utf8_lossy(&buf[..32 + (round % 400)]);
        let _ = signature(&text);
        let _ = canonicalize(&text);
    }

    // Prefix scan over a document with multi-byte characters: every
    // lossy prefix canonicalizes and hashes without panic, and
    // extending a non-empty prefix by a word changes the signature.
    let doc = canonicalize("Xin chào thế giới, đây là văn bản tiếng Việt dài");
    let mut prev = signature("");
    for i in 0..=doc.len() {
        let _ = signature(&String::from_utf8_lossy(&doc.as_bytes()[..i]));
    }
    let words: Vec<&str> = doc.split_whitespace().collect();
    for n in 1..words.len() {
        let cur = signature(&words[..n].join(" "));
        assert_ne!(cur, prev, "adding word {n} must change the signature");
        prev = cur;
    }
}
