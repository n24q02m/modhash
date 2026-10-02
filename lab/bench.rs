//! lab/bench.rs — end-to-end CLI throughput measurement.
//!
//! `modhash bench` measures the library pipeline in-process; THIS file
//! measures the command a user actually runs: process spawn + file I/O +
//! decode + all tiers, wall-clock via `std::time::Instant`. No criterion,
//! no dependencies — compile and run:
//!
//! ```text
//! cargo build --release -p modhash-cli
//! rustc -O lab/bench.rs -o target/release/lab-bench
//! ./target/release/lab-bench [path-to-modhash-binary] [iters]
//! ```
//!
//! Prints one row per fixture (median of `iters` runs). Inputs are the
//! committed test fixtures — every number here is reproducible on the
//! same tree.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// A fixture row: label, repo-relative path.
struct Case {
    name: &'static str,
    file: &'static str,
    cmd: &'static str,
}

const CASES: &[Case] = &[
    Case { name: "png", file: "modhash/tests/fixtures/phash_rgb8_48x40.png", cmd: "describe" },
    Case { name: "jpeg", file: "modhash/tests/fixtures/base_444.jpg", cmd: "describe" },
    Case { name: "wav", file: "modhash/tests/fixtures/tone.wav", cmd: "describe" },
    Case { name: "flac", file: "modhash/tests/fixtures/tone.flac", cmd: "describe" },
    Case { name: "mp3", file: "modhash/tests/fixtures/l3_short.mp3", cmd: "describe" },
    Case { name: "pdf", file: "modhash/tests/fixtures/text_page.pdf", cmd: "describe" },
    Case { name: "wav-hash", file: "modhash/tests/fixtures/tone.wav", cmd: "hash" },
];

fn main() {
    let mut args = std::env::args().skip(1);
    let bin = args.next().unwrap_or_else(|| "target/release/modhash".to_string());
    let bin = if cfg!(windows) && !bin.ends_with(".exe") {
        format!("{bin}.exe")
    } else {
        bin
    };
    let iters: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20).max(1);
    // Fixtures are addressed relative to the current directory — run
    // from the repository root (the documented invocation does).

    if !Path::new(&bin).exists() {
        eprintln!("bench: {bin} not found — build `cargo build --release -p modhash-cli` first");
        std::process::exit(2);
    }
    println!("# modhash end-to-end CLI wall-clock (median of {iters})");
    println!("# bin: {bin}");
    println!(
        "# arch={} os={} release-build={}",
        std::env::consts::ARCH,
        std::env::consts::OS,
        bin.contains("release"),
    );
    for case in CASES {
        if !Path::new(case.file).exists() {
            println!("{:<10} MISSING {}", case.name, case.file);
            continue;
        }
        let size = std::fs::metadata(case.file).map(|m| m.len()).unwrap_or(0);
        let mut times: Vec<f64> = Vec::with_capacity(iters as usize);
        for _ in 0..iters {
            let t = Instant::now();
            let status = Command::new(&bin)
                .arg(case.cmd)
                .arg(case.file)
                .output()
                .expect("spawn modhash");
            let dt = t.elapsed().as_secs_f64();
            if !status.status.success() {
                eprintln!(
                    "bench: `modhash {} {}` exited {}: {}",
                    case.cmd,
                    case.file,
                    status.status,
                    String::from_utf8_lossy(&status.stderr)
                );
                std::process::exit(1);
            }
            times.push(dt);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = times[times.len() / 2];
        println!(
            "{:<10} {:>7} B  median {:>7.2} ms  ({:>6.1} MB/s)",
            case.name,
            size,
            med * 1000.0,
            size as f64 / med / 1e6,
        );
    }
}
