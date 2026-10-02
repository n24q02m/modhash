//! Conformance for `modhash-unicode`: the official UCD `NormalizationTest.txt`
//! (all 19 074 data lines), the Vietnamese decomposed/precomposed pair the
//! spec requires, idempotence, and hash equality on the NFC forms.
//!
//! The conformance corpus is `tests/NormalizationTest.txt`, embedded with
//! `include_bytes!` so the test does not depend on the working directory and
//! stays a fixed contract (`.gitattributes` pins it to LF). Its bytes are
//! generated data, not hand-written code.

use modhash_unicode::{nfc, nfd};

const CORPUS: &str = include_str!("NormalizationTest.txt");

/// Parse a semicolon column of hex code points into a `String`.
fn cps(col: &str) -> String {
    col.split_whitespace()
        .map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap())
        .collect()
}

fn each_line(mut f: impl FnMut(&str, [String; 5])) {
    for raw in CORPUS.lines() {
        let line = raw.split('#').next().unwrap().trim();
        if line.is_empty() || line.starts_with('@') {
            continue;
        }
        let cols: Vec<&str> = line.split(';').map(str::trim).collect();
        assert!(cols.len() >= 5, "short row: {line}");
        let seqs = [
            cps(cols[0]),
            cps(cols[1]),
            cps(cols[2]),
            cps(cols[3]),
            cps(cols[4]),
        ];
        f(line, seqs);
    }
}

/// UAX #15 conformance: toNFC(c1..c3) == c2 and toNFC(c4..c5) == c4; the
/// NFD halves c3 == toNFD(c1..c3) and c5 == toNFD(c4..c5) come for free
/// because this crate ships `nfd` too.
#[test]
fn ucd_conformance() {
    let mut lines = 0;
    each_line(|line, s| {
        lines += 1;
        let expected_nfc = [&s[1], &s[1], &s[1], &s[3], &s[3]];
        let expected_nfd = [&s[2], &s[2], &s[2], &s[4], &s[4]];
        for (src, (&en, &ed)) in s.iter().zip(expected_nfc.iter().zip(&expected_nfd)) {
            assert_eq!(&nfc(src), en, "NFC mismatch on {line}");
            assert_eq!(&nfd(src), ed, "NFD mismatch on {line}");
        }
    });
    assert_eq!(lines, 19_074, "every data line of the corpus must run");
}

/// Idempotence: NFC(NFC(x)) == NFC(x) for every column of every line.
#[test]
fn nfc_idempotent() {
    each_line(|line, s| {
        for src in &s {
            let once = nfc(src);
            assert_eq!(nfc(&once), once, "NFC not idempotent on {line}");
        }
    });
}

/// The Vietnamese pair the spec calls out (§4.1): decomposed
/// `Ta\u{0302}\u{0300}ng` and precomposed `Tầng` must NFC to the same
/// string — the combining marks U+0302 (circumflex) then U+0300 (grave)
/// have equal ccc=230, keep their order, and `a`+circumflex = `â`,
/// `â`+grave = `ầ`.
#[test]
fn vietnamese_pair() {
    let decomposed = "Ta\u{0302}\u{0300}ng";
    let precomposed = "T\u{1EA7}ng";
    assert_eq!(nfc(decomposed), precomposed);
    assert_eq!(nfc(precomposed), precomposed);
    // And the pair the spec warns about: `T\u{0300}` has no precomposed
    // grave form, so it must NOT collapse into `Tầng`.
    assert_ne!(nfc("T\u{0300}\u{1EA7}ng"), precomposed);
}

/// Every code point NOT listed in column 1 of Part 1 of the corpus has all
/// normal forms equal to itself (UAX #15 invariant). This scans every
/// Unicode scalar value, not a sample: the corpus's own list is the oracle
/// for which code points may legitimately change.
#[test]
fn unlisted_codepoints_are_stable() {
    let mut in_part1 = false;
    let mut listed = std::collections::BTreeSet::new();
    for raw in CORPUS.lines() {
        if raw.starts_with("@Part1") {
            in_part1 = true;
            continue;
        }
        if raw.starts_with('@') && in_part1 {
            break;
        }
        if !in_part1 {
            continue;
        }
        let line = raw.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        for h in line.split(';').next().unwrap().split_whitespace() {
            listed.insert(u32::from_str_radix(h, 16).unwrap());
        }
    }
    let mut checked = 0u64;
    for cp in 0u32..=0x10FFFF {
        if listed.contains(&cp) || (0xD800..0xE000).contains(&cp) {
            continue;
        }
        let ch = char::from_u32(cp).unwrap();
        let s = String::from(ch);
        assert_eq!(nfc(&s), s, "unlisted cp {cp:04X} changed under NFC");
        assert_eq!(nfd(&s), s, "unlisted cp {cp:04X} changed under NFD");
        checked += 1;
    }
    assert_eq!(
        checked,
        0x10FFFF + 1 - listed.len() as u64 - 2048,
        "scan must cover every scalar the corpus does not list"
    );
}

/// The kit's tier-1 contract: the decomposed and precomposed forms of the
/// same Vietnamese syllable produce the same NFC bytes, and identical bytes
/// must hash identically under the kit's own SHA-256. The known-answer
/// vector pins the oracle itself, so a broken hasher cannot fake equality.
#[test]
fn nfc_forms_hash_equal() {
    let abc = modhash_primitives::sha256(b"abc").unwrap();
    assert_eq!(
        abc.to_string(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let decomposed = nfc("Ta\u{0302}\u{0300}ng");
    let precomposed = nfc("T\u{1EA7}ng");
    let h1 = modhash_primitives::sha256(decomposed.as_bytes()).unwrap();
    let h2 = modhash_primitives::sha256(precomposed.as_bytes()).unwrap();
    assert_eq!(h1, h2);
}
