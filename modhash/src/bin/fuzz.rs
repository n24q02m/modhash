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
/// The skeleton registers none: no codec has an implementation yet. Each
/// owning phase appends its own name and a matching arm in `dispatch`.
const TARGET_NAMES: &[&str] = &[];

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
        other if TARGET_NAMES.contains(&other) => Err(format!(
            "target {other} is registered but has no corpus wired yet"
        )),
        other => Err(format!("unknown target {other}")),
    }
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

fn usage() -> String {
    let mut s = String::from("usage: fuzz <target> <iters> <seed>\n\ntargets:\n");
    s.push_str("  coremode  mutation engine self-check\n");
    // TARGET_NAMES is a `const` that is currently empty, so on the MSRV
    // toolchain clippy can const-fold this to `true` and rejects the branch
    // as dead code. The check becomes load-bearing the moment the first codec
    // target is registered.
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
