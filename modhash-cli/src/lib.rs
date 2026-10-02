//! Library core of the `modhash` command line interface.
//!
//! The binary (`src/main.rs`) is a thin argument parser over this
//! surface; every observable behavior lives here so tests can drive it
//! deterministically without spawning a process.
//!
//! Part of the `modhash` zero-dependency hashing kit: the only crate
//! this one depends on is the kit facade `modhash` (crate #21), so the
//! whole dependency graph still resolves without a single registry
//! package (`scripts/gate_zero_dep.sh` enforces it).
//!
//! # The one scalar distance
//!
//! `match()` answers a modality-shaped [`MatchOutcome`]. `calibrate`
//! needs a single `u32` axis, so [`distance`] defines the kit's scalar
//! per modality, monotone in dissimilarity:
//!
//! - image → the Hamming distance itself;
//! - audio → `u32::MAX - votes` (more votes, smaller distance);
//! - text / binary → `round((1 − jaccard) × 1e6)`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fmt;
use std::path::{Path, PathBuf};

use modhash::{Description, MatchOutcome, Signature};

/// One input file: path for reports, bytes for the pipelines.
pub struct Input {
    /// Where the bytes were read from (the report name).
    pub path: PathBuf,
    /// Raw file content.
    pub bytes: Vec<u8>,
}

/// Reads `path` into memory.
///
/// # Errors
///
/// `std::io::Error` naming the path when the read fails.
pub fn read_input(path: &Path) -> std::io::Result<Input> {
    let bytes = std::fs::read(path)?;
    Ok(Input {
        path: path.to_path_buf(),
        bytes,
    })
}

/// The tier-2 signature of `bytes`, routed on `modhash::detect`.
///
/// # Errors
///
/// `modhash::Error` — `Unsupported` on a pending slot (mp4 video),
/// `Decode`/`Audio`/`Pdf` on rejected containers.
pub fn signature_of(bytes: &[u8]) -> Result<Signature, modhash::Error> {
    modhash::signature(bytes)
}

/// The scalar `u32` distance between two same-modality signatures —
/// the axis `calibrate` and `--threshold` operate on. Monotone in
/// dissimilarity; see the crate docs for the per-modality definition.
///
/// # Errors
///
/// `modhash::Error::BadValue` when the modalities differ.
pub fn distance(a: &Signature, b: &Signature) -> Result<u32, modhash::Error> {
    match modhash::match_(a, b)? {
        MatchOutcome::Image { hamming, .. } => Ok(hamming),
        MatchOutcome::Audio { best, .. } => Ok(u32::MAX - best.map_or(0, |m| m.votes)),
        MatchOutcome::Text { jaccard, .. } | MatchOutcome::Binary { jaccard, .. } => {
            Ok(((1.0 - jaccard) * 1_000_000.0).round() as u32)
        }
    }
}

/// One line naming the score for a [`MatchOutcome`] — the
/// `score: …` row of `modhash match`.
#[must_use]
pub fn score_line(outcome: &MatchOutcome) -> String {
    match outcome {
        MatchOutcome::Image { hamming, .. } => format!("score: hamming={hamming}"),
        MatchOutcome::Audio { best, .. } => match best {
            Some(m) => format!("score: votes={} delta_t={}", m.votes, m.delta_t),
            None => "score: votes=0".to_string(),
        },
        MatchOutcome::Text { jaccard, .. } => format!("score: jaccard={jaccard:.6}"),
        MatchOutcome::Binary { jaccard, .. } => format!("score: jaccard={jaccard:.6}"),
    }
}

/// The advisory bound each modality's `matched` flag was decided
/// against, spelled out so a threshold never floats unattributed.
#[must_use]
pub fn bound_line(outcome: &MatchOutcome) -> String {
    match outcome {
        MatchOutcome::Image { .. } => "advisory: hamming <= 10".to_string(),
        MatchOutcome::Audio { .. } => "advisory: votes >= 8".to_string(),
        MatchOutcome::Text { .. } => "advisory: jaccard >= 0.8".to_string(),
        MatchOutcome::Binary { .. } => "advisory: jaccard >= 0.5".to_string(),
    }
}

/// Whether `outcome` carries `matched: true`.
#[must_use]
pub fn is_match(outcome: &MatchOutcome) -> bool {
    match outcome {
        MatchOutcome::Image { matched, .. }
        | MatchOutcome::Audio { matched, .. }
        | MatchOutcome::Text { matched, .. }
        | MatchOutcome::Binary { matched, .. } => *matched,
    }
}

/// Whether `distance(a, b)` does not exceed `threshold` — or the
/// advisory verdict when `threshold` is [`None`]. This comparison is
/// the dedup clustering edge predicate.
fn close_enough(a: &Signature, b: &Signature, threshold: Option<u32>) -> bool {
    match threshold {
        Some(t) => distance(a, b).is_ok_and(|d| d <= t),
        None => modhash::match_(a, b).is_ok_and(|o| is_match(&o)),
    }
}

/// One duplicate cluster: member indices into the input slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    /// Indices of the clustered inputs, in input order.
    pub members: Vec<usize>,
    /// `true` when every member shares one tier-1 digest — an exact
    /// duplicate set, not merely a perceptual one.
    pub exact: bool,
}

/// The result of a `dedup` pass over one directory's inputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DedupReport {
    /// Clusters of ≥ 2 members, sorted by first member index.
    pub groups: Vec<Cluster>,
    /// Inputs that formed no cluster.
    pub singles: Vec<usize>,
    /// Inputs the signature pipeline refused (pending or corrupt),
    /// reported by index so the report never silently drops a file.
    pub skipped: Vec<usize>,
}

/// Clusters `inputs` by tier-2 signature: union-find over pairs whose
/// verdict matches (advisory bound, or `distance <= threshold` when a
/// calibrated threshold is passed). Deterministic: equal inputs yield
/// equal groups on every platform.
#[must_use]
pub fn dedup_inputs(inputs: &[Input], threshold: Option<u32>) -> DedupReport {
    let mut report = DedupReport::default();

    // Pass 1: one signature + tier-1 digest per input, errors parked in
    // `skipped` with their index — a corrupt file never aborts the run.
    let mut sigs: Vec<Option<(Signature, modhash::Digest<32>)>> = Vec::with_capacity(inputs.len());
    for (i, input) in inputs.iter().enumerate() {
        match modhash::describe(&input.bytes) {
            Ok(d) => sigs.push(Some((d.signature, d.tier1))),
            Err(_) => {
                sigs.push(None);
                report.skipped.push(i);
            }
        }
    }

    // Pass 2: union-find over same-modality pairs that satisfy the
    // clustering predicate.
    let n = inputs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for i in 0..n {
        for j in (i + 1)..n {
            let (Some((sa, _)), Some((sb, _))) = (&sigs[i], &sigs[j]) else {
                continue;
            };
            if sa.modality() != sb.modality() {
                continue;
            }
            if close_enough(sa, sb, threshold) {
                let (ra, rb) = (root(&mut parent, i), root(&mut parent, j));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
    }

    // Pass 3: gather clusters keyed by root; members stay in input
    // order, groups sort by smallest member index.
    let mut by_root: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, sig) in sigs.iter().enumerate() {
        if sig.is_none() {
            continue;
        }
        let r = root(&mut parent, i);
        match by_root.iter_mut().find(|(root, _)| *root == r) {
            Some((_, members)) => members.push(i),
            None => by_root.push((r, vec![i])),
        }
    }
    by_root.sort_by_key(|(r, _)| *r);
    for (_, members) in by_root {
        if members.len() == 1 {
            report.singles.push(members[0]);
            continue;
        }
        let first_tier1 = sigs[members[0]].as_ref().map(|(_, t)| *t);
        let exact = members
            .iter()
            .all(|&m| sigs[m].as_ref().map(|(_, t)| *t) == first_tier1);
        report.groups.push(Cluster { members, exact });
    }
    report
}

/// Renders a [`Description`] for the `describe` command: the facade
/// `Display` plus an explicit `tier3:` line — the describe contract is
/// three tiers on every input, so a modality without local features
/// says so instead of omitting the row.
#[must_use]
pub fn render_description(desc: &Description) -> String {
    let tier3 = match &desc.facts {
        modhash::Facts::Image { .. } => None, // facade prints keypoints
        modhash::Facts::Audio { .. } => Some("tier3: dtw (no stored features; comparator only)"),
        modhash::Facts::Text { .. } => Some("tier3: none (docx lane shares text tier-2)"),
        modhash::Facts::Binary { .. } => Some("tier3: none (no local features for binary)"),
    };
    let mut out = desc.to_string();
    if let Some(line) = tier3 {
        // Insert after the tier2 line: `t2` is the newline BEFORE
        // "tier2:", so the row ends at the next newline after t2+1.
        match out.find("\ntier2:") {
            Some(t2) => match out[t2 + 1..].find('\n') {
                Some(nl) => {
                    out.insert_str(t2 + 1 + nl + 1, &format!("{line}\n"));
                    out
                }
                None => format!("{out}\n{line}"),
            },
            None => format!("{out}\n{line}"),
        }
    } else {
        out
    }
}

/// JSON-escapes `s` into `out` (the six required escapes plus control
/// characters); every string the CLI emits passes through here.
pub fn json_escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

/// `json_escape_into` with the quotes included.
#[must_use]
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    json_escape_into(s, &mut out);
    out.push('"');
    out
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path.display().to_string())
    }
}

// ---------------------------------------------------------------------
// Deterministic fixture synthesis (shared by tests and `bench`)
// ---------------------------------------------------------------------

/// Deterministic input generation for `bench` and for tests that need
/// real container bytes without depending on fixture files. Every
/// function is pure: the same arguments produce the same bytes on every
/// platform, always.
pub mod fixture {
    /// SplitMix64: a counter-based PRNG — the kit's deterministic seed
    /// expansion (`modhash-primitives` uses the same recurrence).
    pub struct SplitMix64 {
        state: u64,
    }

    impl SplitMix64 {
        /// Seeds the generator.
        #[must_use]
        pub fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        /// The next 64-bit value.
        pub fn next_u64(&mut self) -> u64 {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// The next byte.
        pub fn next_u8(&mut self) -> u8 {
            (self.next_u64() >> 32) as u8
        }
    }

    /// A `w`×`h` RGB pixel buffer of smooth periodic content — the low
    /// frequencies a pHash responds to, so distinct seeds yield distinct
    /// hashes while small perturbations stay inside a cluster.
    #[must_use]
    pub fn patterned_pixels(seed: u64, w: u32, h: u32) -> Vec<u8> {
        let mut rng = SplitMix64::new(seed);
        // Per-seed phase/frequency knobs give each base image its own
        // low-frequency signature.
        let fx = (rng.next_u64() % 5 + 1) as f64;
        let fy = (rng.next_u64() % 5 + 1) as f64;
        let p1 = (rng.next_u64() % 628) as f64 / 100.0;
        let p2 = (rng.next_u64() % 628) as f64 / 100.0;
        let mut px = Vec::with_capacity(w as usize * h as usize * 3);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let u = x as f64 / w as f64;
                let v = y as f64 / h as f64;
                let r = 127.5 + 120.0 * (u * fx * std::f64::consts::TAU + p1).sin();
                let g = 127.5 + 120.0 * (v * fy * std::f64::consts::TAU + p2).sin();
                let b = 127.5 + 120.0 * ((u + v) * (fx + fy) + p1 + p2).sin();
                px.extend_from_slice(&[r as u8, g as u8, b as u8]);
            }
        }
        px
    }

    /// Perturbs `pixels` (RGB triples) by ±`amount` on ~1/`per` pixels,
    /// seeded — a "variant" of one base image.
    #[must_use]
    pub fn perturb_pixels(pixels: &[u8], seed: u64, amount: u8, per: u64) -> Vec<u8> {
        let mut rng = SplitMix64::new(seed);
        let per = per.max(1);
        let mut out = pixels.to_vec();
        for (i, b) in out.iter_mut().enumerate() {
            let r = rng.next_u64();
            if i > 0 && r % per == 0 {
                let d = (r >> 8) as u8 % (amount.saturating_mul(2) + 1);
                *b = b.saturating_add(d).saturating_sub(amount);
            }
        }
        out
    }

    /// Encodes RGB triples (`3·w·h` bytes) as a bottom-up 24-bit BMP.
    #[must_use]
    pub fn bmp24(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
        debug_assert_eq!(rgb.len(), (w * h * 3) as usize);
        let row = (w as usize * 3).div_ceil(4) * 4;
        let img = row * h as usize;
        let mut f = Vec::with_capacity(54 + img);
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&(54 + img as u32).to_le_bytes());
        f.extend_from_slice(&[0; 4]); // reserved
        f.extend_from_slice(&54u32.to_le_bytes()); // pixel offset
        f.extend_from_slice(&40u32.to_le_bytes()); // DIB header
        f.extend_from_slice(&w.to_le_bytes());
        f.extend_from_slice(&h.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes()); // planes
        f.extend_from_slice(&24u16.to_le_bytes()); // bpp
        f.extend_from_slice(&[0; 24]); // compression + sizes + ppm + colors
        for y in (0..h as usize).rev() {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 3;
                f.extend_from_slice(&[rgb[i + 2], rgb[i + 1], rgb[i]]); // BGR
            }
            f.resize(f.len() + (row - w as usize * 3), 0);
        }
        f
    }

    /// Encodes mono 16-bit PCM as a minimal RIFF/WAVE buffer.
    #[must_use]
    pub fn wav16(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data = (samples.len() * 2) as u32;
        let mut f = Vec::with_capacity(44 + data as usize);
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        f.extend_from_slice(&1u16.to_le_bytes()); // PCM
        f.extend_from_slice(&1u16.to_le_bytes()); // mono
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
        f.extend_from_slice(&2u16.to_le_bytes()); // block align
        f.extend_from_slice(&16u16.to_le_bytes()); // bits
        f.extend_from_slice(b"data");
        f.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            f.extend_from_slice(&s.to_le_bytes());
        }
        f
    }

    /// A deterministic ~`seconds`-long mono 16-bit 44 100 Hz tone
    /// (three harmonics, seeded detune) for audio-lane inputs.
    #[must_use]
    pub fn tone(seed: u64, seconds: f64) -> Vec<i16> {
        let mut rng = SplitMix64::new(seed);
        let base = 220.0 + (rng.next_u64() % 220) as f64;
        let n = (44_100.0 * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / 44_100.0;
                let s = 0.6 * (t * base * std::f64::consts::TAU).sin()
                    + 0.25 * (t * base * 2.0 * std::f64::consts::TAU).sin()
                    + 0.15 * (t * base * 3.7 * std::f64::consts::TAU).sin();
                (s * 20_000.0) as i16
            })
            .collect()
    }

    /// Deterministic ~`bytes`-sized binary content: 4 KiB seeded blocks
    /// with a seeded splice every other block so FastCDC sees real
    /// content-defined boundaries.
    #[must_use]
    pub fn binary_blob(seed: u64, bytes: usize) -> Vec<u8> {
        let mut rng = SplitMix64::new(seed);
        let mut out = Vec::with_capacity(bytes);
        let mut block = 0;
        while out.len() < bytes {
            let take = (bytes - out.len()).min(4096);
            for i in 0..take {
                out.push(if block % 3 == 1 {
                    rng.next_u8()
                } else {
                    (i as u32 ^ (block * 97)) as u8
                });
            }
            block += 1;
        }
        out
    }

    /// Deterministic ~`bytes`-sized text: words drawn from a small
    /// seeded vocabulary, one space separated.
    #[must_use]
    pub fn prose(seed: u64, bytes: usize) -> String {
        const VOCAB: [&str; 24] = [
            "hash", "image", "audio", "signal", "codec", "block", "frame", "sample", "stream",
            "vector", "median", "kernel", "window", "raster", "pixel", "tone", "hash", "luma",
            "delta", "chunk", "radix", "filter", "table", "edge",
        ];
        let mut rng = SplitMix64::new(seed);
        let mut out = String::with_capacity(bytes + 16);
        while out.len() < bytes {
            out.push_str(VOCAB[(rng.next_u64() as usize) % VOCAB.len()]);
            out.push(' ');
        }
        out.truncate(bytes);
        out
    }
}
