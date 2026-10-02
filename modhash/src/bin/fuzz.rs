//! Deterministic fuzz harness for the `modhash` workspace.
//!
//! One binary owns fuzzing for every crate, because the `modhash` crate sits at
//! the top of the dependency DAG and can see all of them:
//!
//! ```text
//! cargo run --release -p modhash --bin fuzz -- <target> <iters> <seed>
//! ```
//!
//! Everything is driven by a splitmix64 PRNG seeded from the command line, so
//! a failing iteration number reproduces exactly on any machine.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
/// The four mutation modes every target is fuzzed with.
const MODES: [&str; 4] = ["random", "truncate", "bitflip", "repeat-insert"];

/// splitmix64: the reference finaliser from Steele et al. (2014).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a generator from `seed`.
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next 64-bit output.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value in `0..n`, or `0` when `n` is zero.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
}

/// Codec fuzz targets, registered by the phase that owns each codec.
///
/// Each owning phase appends its own name and a matching arm in
/// `dispatch`, and seeds `fuzz/corpus/<name>/` when its corpus is
/// file-based. Embedded-seed targets keep their seed in the arm.
/// `png`/`jpeg`/`bmp`/`audio` were seeded by the facade phase — see
/// `fuzz/corpus/PROVENANCE.md`.
const TARGET_NAMES: &[&str] = &["mp4", "h264", "flac", "png", "jpeg", "bmp", "audio"];

fn apply(mode: &str, rng: &mut SplitMix64, input: &[u8]) -> Vec<u8> {
    match mode {
        "random" => (0..=rng.below(512)).map(|_| rng.next_u64() as u8).collect(),
        "truncate" => {
            let end = rng.below(input.len() + 1);
            input[..end].to_vec()
        }
        "bitflip" => {
            // An empty input has no bit to flip. Returning it unchanged is the
            // honest semantic: this mode corrupts existing bytes and never
            // introduces new ones. Truncate can reach length zero, so this
            // case is reachable, not hypothetical.
            if input.is_empty() {
                return input.to_vec();
            }
            let mut out = input.to_vec();
            let flips = 1 + rng.below(8);
            for _ in 0..flips {
                let i = rng.below(out.len());
                out[i] ^= 1 << rng.below(8);
            }
            out
        }
        "repeat-insert" => {
            if input.is_empty() {
                return vec![rng.next_u64() as u8];
            }
            let at = rng.below(input.len());
            let take = 1 + rng.below(input.len());
            let end = at.saturating_add(take).min(input.len());
            let mut out = input[..at].to_vec();
            out.extend_from_slice(&input[at..end]);
            out.extend_from_slice(&input[at..]);
            out
        }
        other => unreachable!("unknown mutation mode {other}"),
    }
}
/// Routes `target` to its phase implementation.
fn dispatch(target: &str, rng: &mut SplitMix64, iters: usize, seed: u64) -> Result<(), String> {
    match target {
        "coremode" => fuzz_coremode(rng, iters, seed),
        "mp4" => fuzz_codec(rng, iters, seed, "mp4", |bytes| {
            // The container demuxer plus the video lane above it:
            // demux, the facade signature path and the full fingerprint
            // all see every mutated input.
            modhash_mp4::demux(bytes)?;
            let _ = modhash::signature(bytes);
            modhash_video::decode(bytes, &modhash_video::Limits::default()).map(|_| ())
        }),
        "h264" => fuzz_codec(rng, iters, seed, "h264", |bytes| {
            modhash_h264::decode(bytes).map(|_| ())
        }),
        "flac" => fuzz_flac(rng, iters, seed),
        "png" => fuzz_codec(rng, iters, seed, "png", |bytes| {
            // The decoder plus the facade's image lane above it.
            let _ = modhash::signature(bytes);
            modhash_png::decode(bytes, &modhash_png::Limits::default()).map(|_| ())
        }),
        "jpeg" => fuzz_codec(rng, iters, seed, "jpeg", |bytes| {
            let _ = modhash::signature(bytes);
            modhash_jpeg::decode(bytes).map(|_| ())
        }),
        "bmp" => fuzz_codec(rng, iters, seed, "bmp", |bytes| {
            let _ = modhash::signature(bytes);
            modhash_bmp::decode(bytes).map(|_| ())
        }),
        "audio" => fuzz_codec(rng, iters, seed, "audio", |bytes| {
            // Every audio entry point sees every mutated input: both
            // decoders and both signature facades, plus the facade.
            let _ = modhash::signature(bytes);
            let _ = modhash_wav::decode(bytes);
            let _ = modhash_flac::decode(bytes, &modhash_flac::Limits::default());
            let _ = modhash_mp3::decode(bytes, &modhash_mp3::Limits::default());
            let _ = modhash_audio::signature_of_wav(bytes);
            let _ = modhash_audio::signature_of_flac(bytes);
            Ok(())
        }),
        other => Err(format!("unknown target {other}")),
    }
}

/// Generic codec fuzz loop: seed corpus from `fuzz/corpus/<target>/` (one
/// file per case), mutate each seed through the four modes in round-robin,
/// run the decoder, and report panics with the reproducing iteration and
/// seed. Decoders signal corrupt input with `Err`, which is not a failure.
fn fuzz_codec(
    rng: &mut SplitMix64,
    iters: usize,
    seed: u64,
    target: &str,
    decode: impl Fn(&[u8]) -> Result<(), modhash_primitives::Error> + std::panic::RefUnwindSafe,
) -> Result<(), String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fuzz")
        .join("corpus")
        .join(target);
    let mut corpus: Vec<Vec<u8>> = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| format!("corpus dir {} unreadable: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_file() {
            corpus.push(std::fs::read(&path).map_err(|e| format!("{e}"))?);
        }
    }
    if corpus.is_empty() {
        return Err(format!("corpus dir {} has no seed files", dir.display()));
    }
    for i in 0..iters {
        let base = &corpus[i % corpus.len()];
        let input = apply(MODES[i % MODES.len()], rng, base);
        // Panics inside the decoder are the failure the harness exists to
        // catch; catch_unwind turns them into a named crash report.
        let outcome = std::panic::catch_unwind(|| decode(&input));
        if outcome.is_err() {
            return Err(format!("{target} panicked at iter {i} (seed {seed})"));
        }
    }
    Ok(())
}

/// Fuzzes the mutation engine itself.
///
/// Every codec target is added later; this one exists so the PRNG, the four
/// mutation modes and the iteration loop are themselves verified from the
/// first commit. It asserts the property each mode is supposed to guarantee,
/// so a broken mutator fails here instead of silently weakening every codec
/// target that will be added on top of it.
fn fuzz_coremode(rng: &mut SplitMix64, iters: usize, seed: u64) -> Result<(), String> {
    // Two corpora. The first is seeded non-empty so truncate walks the corpus
    // down through its usual lengths. The second starts empty, so the
    // degenerate paths - mutate nothing, insert into nothing - are exercised
    // from iteration zero on every seed, rather than only when a seed happens
    // to truncate to zero first.
    let seeds: [&[u8]; 2] = [&(0..64u8).collect::<Vec<u8>>(), &[]];
    for (ci, base) in seeds.into_iter().enumerate() {
        let mut corpus: Vec<u8> = base.to_vec();
        for i in 0..iters {
            let mode = MODES[i % MODES.len()];
            let before = corpus.len();
            corpus = apply(mode, rng, &corpus);
            let after = corpus.len();
            let ok = match mode {
                // Length is chosen freely, but never unbounded.
                "random" => after <= 512,
                "truncate" => after <= before,
                // Length preserving, including zero -> zero.
                "bitflip" => after == before,
                "repeat-insert" => after >= before,
                _ => false,
            };
            if !ok {
                return Err(format!(
                    "mode {mode} broke its invariant at iter {i} of corpus {ci}: \
                     len {before} -> {after} (seed {seed})"
                ));
            }
        }
        let digest = corpus.iter().fold(0u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01B3)
        });
        println!(
            "coremode {ci}: {iters} iters, seed {seed}, final corpus {} B, digest {digest:#018x}",
            corpus.len()
        );
    }
    Ok(())
}

/// Fuzzes `modhash-flac`'s decoder entry point.
///
/// The corpus starts from a complete, checksum-valid stream (a mono
/// 16-bit 44.1 kHz stream with one 8-sample verbatim frame, built
/// bit-for-bit by the crate's own test harness) and is mutated in place,
/// so most iterations land inside the format rather than bouncing off
/// the `fLaC` signature check. The decoder's contract is that every
/// input returns `Ok` or `Err` - the only hard failure is a panic, so
/// each call runs inside `catch_unwind` to turn one into a located
/// report instead of an abort.
fn fuzz_flac(rng: &mut SplitMix64, iters: usize, seed: u64) -> Result<(), String> {
    /// A complete valid stream: fLaC + STREAMINFO + one verbatim frame.
    const SEED_STREAM: &[u8] = &[
        0x66, 0x4c, 0x61, 0x43, 0x80, 0x00, 0x00, 0x22, 0x00, 0xc0, 0x12, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x0a, 0xc4, 0x40, 0xf0, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xf8, 0x60, 0x00, 0x00, 0x07, 0xff,
        0x02, 0x00, 0x01, 0xff, 0xfe, 0x00, 0x03, 0xff, 0xfc, 0x00, 0x05, 0xff, 0xfa, 0x00, 0x07,
        0xff, 0xf8, 0xaf, 0x0b,
    ];
    let mut corpus: Vec<u8> = SEED_STREAM.to_vec();
    for i in 0..iters {
        let mode = MODES[i % MODES.len()];
        corpus = apply(mode, rng, &corpus);
        // Mutations can grow the corpus; cap it so the decode stays
        // inside `modhash_flac::Limits::default().max_input` and every
        // iteration exercises parsing, not the input-length ceiling.
        if corpus.len() > 1024 {
            corpus.truncate(1024);
        }
        let input = corpus.clone();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            modhash_flac::decode(&input, &modhash_flac::Limits::default())
        }));
        if outcome.is_err() {
            return Err(format!(
                "flac panicked at iter {i} (mode {mode}, seed {seed}), input {} B",
                corpus.len()
            ));
        }
    }
    println!("flac: {iters} iters, seed {seed}, no panic");
    Ok(())
}

fn usage() -> String {
    let mut s = String::from("usage: fuzz <target> <iters> <seed>\n\ntargets:\n");
    s.push_str("  coremode  mutation engine self-check\n");
    // TARGET_NAMES is a `const`, so on the MSRV toolchain clippy can
    // const-fold `is_empty()` and reject the branch as dead code while
    // no codec target is registered. The check becomes load-bearing
    // with the first registration; the allow keeps the empty state
    // compilable and the populated state honest.
    #[allow(clippy::const_is_empty)]
    if TARGET_NAMES.is_empty() {
        s.push_str("  (no codec targets registered yet - each codec phase adds its own)\n");
    } else {
        for name in TARGET_NAMES {
            let _ = writeln!(s, "  {name}");
        }
    }
    let _ = write!(s, "\nmodes: {}\n", MODES.join(", "));
    s
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprint!("{}", usage());
        return ExitCode::from(2);
    }

    let target = args[1].as_str();
    let iters: usize = match args[2].parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("iters must be a non-negative integer, got {:?}", args[2]);
            return ExitCode::from(2);
        }
    };
    let seed: u64 = match args[3].parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("seed must be a u64, got {:?}", args[3]);
            return ExitCode::from(2);
        }
    };

    let mut rng = SplitMix64::new(seed);
    match dispatch(target, &mut rng, iters, seed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fuzz failed: {e}");
            eprint!("{}", usage());
            ExitCode::FAILURE
        }
    }
}
