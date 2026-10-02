//! The `modhash` command line interface.
//!
//! Command surface is specified in design spec §4.5 and implemented in phase
//! P18. Until then the binary answers truthfully about its own state rather
//! than pretending to hash anything.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::process::ExitCode;

/// Commands that design spec §4.5 commits the binary to shipping.
const PLANNED: [&str; 6] = ["describe", "hash", "match", "dedup", "bench", "calibrate"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version") | Some("--version") => {
            println!("modhash {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(cmd) => {
            eprintln!("modhash: `{cmd}` is specified but not implemented yet (phase P18).\n");
            eprintln!("planned commands: {}", PLANNED.join(", "));
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
modhash - zero-dependency perceptual and content hashing kit

usage: modhash <command>

commands:
  version    print the crate version
  help       print this text

commands still to be implemented in phase P18:
  describe <file>       print the three tiers for one input
  hash <file>           print the canonical hash
  match <a> <b>         compare two inputs across modalities
  dedup <dir>           cluster a directory by canonical hash
  bench                 run the lab benchmarks
  calibrate             fit thresholds on a labelled set
";
