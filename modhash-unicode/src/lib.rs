//! Unicode NFC normalisation driven by tables generated from the UCD.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table, so the whole kit
//! resolves without a single registry package.
//!
//! The tables are generated offline by `lab/gen_unicode_tables.py` from the
//! Unicode Character Database 15.1.0 (provenance in `lab/ucd/PROVENANCE.md`)
//! and embedded as `data/ucd.bin` via `include_bytes!` — no `build.rs`.
//!
//! # `ucd.bin` layout — all integers little-endian
//!
//! | offset | contents |
//! |--------|----------|
//! | 0      | magic `UCD1` (4 B) |
//! | 4      | UCD version `u16` major / minor / update (6 B) |
//! | 10     | max decomposition length `u16`, always 3 (2 B) |
//! | 12     | record counts `u32`: ccc, decomposition, pair (12 B) |
//! | 24     | ccc table: `cp u32, ccc u8, nfc_qc u8` — 6 B/record |
//! | …      | decomposition table: `cp u32, len u8, elems[3] u32` — 17 B |
//! | …      | composition table: `first u32, second u32, composite u32` — 12 B |
//!
//! Every table is sorted by key and searched by binary search. Hangul
//! syllables and jamo are handled algorithmically (UAX #15 §3.12) and never
//! appear in the tables. `nfc_qc` is the NFC_Quick_Check property encoded as
//! 0=Yes, 1=No, 2=Maybe; a string whose every character is `Yes` is already
//! NFC, which is the fast path [`nfc`] takes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// The embedded UCD tables (UCD 15.1.0, see `lab/ucd/PROVENANCE.md`).
const UCD: &[u8] = include_bytes!("../data/ucd.bin");

// Hangul algorithmic constants, UAX #15 §3.12.
const SBASE: u32 = 0xAC00;
const LBASE: u32 = 0x1100;
const VBASE: u32 = 0x1161;
const TBASE: u32 = 0x11A7;
const LCOUNT: u32 = 19;
const VCOUNT: u32 = 21;
const TCOUNT: u32 = 28;
const NCOUNT: u32 = VCOUNT * TCOUNT;
const SCOUNT: u32 = LCOUNT * NCOUNT;

const HEADER: usize = 24;
const MAX_DECOMP: usize = 3;

const fn u32_at(b: &[u8], i: usize) -> u32 {
    (b[i] as u32) | ((b[i + 1] as u32) << 8) | ((b[i + 2] as u32) << 16) | ((b[i + 3] as u32) << 24)
}

// Fail at compile time if the embedded blob is not the UCD 15.1.0 table the
// decoder was written for, or if its records are not the fixed shape below.
// One u32 compare per field group: bytes 4..8 = major|minor, 8..12 =
// update|maxlen, all little-endian.
const _: () = {
    assert!(
        u32_at(UCD, 0) == 0x31_44_43_55,
        "ucd.bin magic must be UCD1"
    );
    assert!(
        u32_at(UCD, 4) == 15 | (1 << 16),
        "ucd.bin must be generated from UCD 15.1.0"
    );
    assert!(
        u32_at(UCD, 8) == (MAX_DECOMP as u32) << 16,
        "ucd.bin must be UCD update 0 with 3-element decomposition records"
    );
};

const CCC_COUNT: usize = u32_at(UCD, 12) as usize;
const DECOMP_COUNT: usize = u32_at(UCD, 16) as usize;
const PAIR_COUNT: usize = u32_at(UCD, 20) as usize;
const CCC_OFF: usize = HEADER;
const DECOMP_OFF: usize = CCC_OFF + CCC_COUNT * 6;
const PAIR_OFF: usize = DECOMP_OFF + DECOMP_COUNT * 17;

/// The canonical combining class of `cp`; 0 when the table has no entry.
fn ccc(cp: u32) -> u8 {
    let (mut lo, mut hi) = (0usize, CCC_COUNT);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let i = CCC_OFF + mid * 6;
        match cp.cmp(&u32_at(UCD, i)) {
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
            core::cmp::Ordering::Equal => return UCD[i + 4],
        }
    }
    0
}

/// The NFC_Quick_Check flag of `cp`: 0=Yes, 1=No, 2=Maybe.
fn nfc_qc(cp: u32) -> u8 {
    let (mut lo, mut hi) = (0usize, CCC_COUNT);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let i = CCC_OFF + mid * 6;
        match cp.cmp(&u32_at(UCD, i)) {
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
            core::cmp::Ordering::Equal => return UCD[i + 5],
        }
    }
    0
}

/// The canonical decomposition of `cp` as `(length, elements)`, or `None`.
/// Table records hold up to [`MAX_DECOMP`] elements; unused slots are 0.
fn decomp(cp: u32) -> Option<(u8, [u32; MAX_DECOMP])> {
    let (mut lo, mut hi) = (0usize, DECOMP_COUNT);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let i = DECOMP_OFF + mid * 17;
        match cp.cmp(&u32_at(UCD, i)) {
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
            core::cmp::Ordering::Equal => {
                let len = UCD[i + 4];
                let elems = [u32_at(UCD, i + 5), u32_at(UCD, i + 9), u32_at(UCD, i + 13)];
                return Some((len, elems));
            }
        }
    }
    None
}

/// The primary composite of `first` + `second`, or `None` — Hangul jamo pairs
/// are resolved algorithmically by the caller, not by this table.
fn composite(first: u32, second: u32) -> Option<u32> {
    let (mut lo, mut hi) = (0usize, PAIR_COUNT);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let i = PAIR_OFF + mid * 12;
        match (first, second).cmp(&(u32_at(UCD, i), u32_at(UCD, i + 4))) {
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
            core::cmp::Ordering::Equal => return Some(u32_at(UCD, i + 8)),
        }
    }
    None
}

/// Recursive canonical decomposition of one code point (UAX #15 §3.7):
/// table decompositions, algorithmic Hangul, and the identity for the rest.
fn decompose(cp: u32, out: &mut Vec<u32>) {
    if (SBASE..SBASE + SCOUNT).contains(&cp) {
        let i = cp - SBASE;
        out.push(LBASE + i / NCOUNT);
        out.push(VBASE + (i % NCOUNT) / TCOUNT);
        let t = i % TCOUNT;
        if t > 0 {
            out.push(TBASE + t);
        }
    } else if let Some((len, elems)) = decomp(cp) {
        for &e in &elems[..len as usize] {
            decompose(e, out);
        }
    } else {
        out.push(cp);
    }
}

/// Canonical ordering (UAX #15 §3.11): stable insertion sort of each run of
/// non-starters by combining class; starters (ccc 0) are never reordered.
fn canonical_order(seq: &mut [u32]) {
    for i in 1..seq.len() {
        let c = ccc(seq[i]);
        if c == 0 {
            continue;
        }
        let mut j = i;
        while j > 0 {
            let pc = ccc(seq[j - 1]);
            if pc == 0 || pc <= c {
                break;
            }
            seq.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// The canonical composition of `first` + `second` if it exists: Hangul
/// L+V and LV+T pairs compose algorithmically, everything else composes only
/// through the pair table (which already excludes composition exclusions).
fn try_compose(first: u32, second: u32) -> Option<u32> {
    if (LBASE..LBASE + LCOUNT).contains(&first) && (VBASE..VBASE + VCOUNT).contains(&second) {
        return Some(SBASE + (first - LBASE) * NCOUNT + (second - VBASE) * TCOUNT);
    }
    let is_lv = (SBASE..SBASE + SCOUNT).contains(&first) && (first - SBASE) % TCOUNT == 0;
    if is_lv && (TBASE + 1..TBASE + TCOUNT).contains(&second) {
        return Some(first + (second - TBASE));
    }
    composite(first, second)
}

/// NFD — canonical decomposition followed by canonical ordering (UAX #15).
#[must_use]
pub fn nfd(input: &str) -> String {
    let mut seq = Vec::with_capacity(input.len());
    for ch in input.chars() {
        decompose(ch as u32, &mut seq);
    }
    canonical_order(&mut seq);
    seq.iter()
        .map(|&cp| char::from_u32(cp).expect("the UCD never decomposes to a surrogate"))
        .collect()
}

/// NFC quick check (UAX #15, "Detecting Normalization Forms"): scans without
/// allocating and answers `false` the moment the text provably may not be
/// NFC — a combining class out of canonical order, or any character whose
/// NFC_Quick_Check is No or Maybe. Maybe is unconditionally pessimistic: a
/// Maybe mark that "fails" to compose can still reorder against pieces of a
/// preceding composite's own decomposition, so only the slow path is sound.
/// `true` means the slow path below would return the input unchanged.
fn is_nfc(input: &str) -> bool {
    let mut last_cc = 0u8;
    for ch in input.chars() {
        let cp = ch as u32;
        let cc = ccc(cp);
        if cc != 0 && cc < last_cc {
            return false; // not in canonical order
        }
        if nfc_qc(cp) != 0 {
            return false; // NFC_Quick_Check is No or Maybe
        }
        last_cc = cc;
    }
    true
}

/// NFC — the canonical composition of [`nfd`]: after ordering, each starter
/// greedily absorbs every following non-starter that can compose with it and
/// is not *blocked* (a character is blocked by a preceding non-composed
/// character whose ccc is >= its own).
#[must_use]
pub fn nfc(input: &str) -> String {
    if is_nfc(input) {
        return input.to_owned();
    }
    let mut seq = Vec::with_capacity(input.len());
    for ch in input.chars() {
        decompose(ch as u32, &mut seq);
    }
    canonical_order(&mut seq);

    let mut out: Vec<u32> = Vec::with_capacity(seq.len());
    let mut starter: Option<usize> = None;
    let mut last_cc = 0u8;
    for &cp in &seq {
        let cc = ccc(cp);
        let composed = starter.and_then(|s| {
            // A character is blocked from the starter by an intervening
            // non-composed character with ccc >= cc (cc==0 can never block).
            if last_cc == 0 || last_cc < cc {
                try_compose(out[s], cp)
            } else {
                None
            }
        });
        if let Some(c) = composed {
            out[starter.expect("composed implies a starter")] = c;
        } else {
            out.push(cp);
            if cc == 0 {
                starter = Some(out.len() - 1);
            }
            last_cc = cc;
        }
    }
    out.iter()
        .map(|&cp| char::from_u32(cp).expect("the UCD never composes to a surrogate"))
        .collect()
}
