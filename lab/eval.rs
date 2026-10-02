//! lab/eval.rs — accuracy evaluation against labelled pair sets.
//!
//! Reads a pairs file (`<a>,<b>,<label>` per line, `#` comments) — the
//! same format `modhash calibrate` consumes — runs `modhash match
//! --json` on each pair, and reports precision/recall/F1 at both the
//! advisory bound and, when `--threshold N` is passed, at the scalar
//! bound N. No dependencies; compile and run:
//!
//! ```text
//! cargo build --release -p modhash-cli
//! rustc -O lab/eval.rs -o target/release/lab-eval
//! ./target/release/lab-eval pairs.csv [path-to-modhash-binary] [--threshold N]
//! ```
//!
//! A pair the pipeline refuses counts as `skipped` and is reported —
//! never silently counted as a true or false anything.

use std::path::Path;
use std::process::Command;

struct Pair {
    a: String,
    b: String,
    label: bool,
}

fn parse_label(s: &str) -> Option<bool> {
    match s.trim() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Pulls `"distance": N` out of the `modhash match --json` output —
/// a tiny extractor, not a JSON parser, because the field is fixed by
/// construction and a dependency for one key is not a dependency.
fn json_distance(s: &str) -> Option<u64> {
    let key = "\"distance\": ";
    let i = s.find(key)? + key.len();
    let end = s[i..].find(|c: char| !c.is_ascii_digit())? + i;
    s[i..end].parse().ok()
}

fn json_matched(s: &str) -> Option<bool> {
    if s.contains("\"matched\": true") {
        Some(true)
    } else if s.contains("\"matched\": false") {
        Some(false)
    } else {
        None
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(csv) = args.next() else {
        eprintln!("usage: lab-eval <pairs.csv> [modhash-binary] [--threshold N]");
        std::process::exit(2);
    };
    let mut bin = "target/release/modhash".to_string();
    let mut threshold: Option<u64> = None;
    let mut it = args.peekable();
    while let Some(a) = it.next() {
        if a == "--threshold" {
            threshold = it.next().and_then(|v| v.parse().ok());
        } else {
            bin = a;
        }
    }
    if cfg!(windows) && !bin.ends_with(".exe") {
        bin.push_str(".exe");
    }
    if !Path::new(&bin).exists() {
        eprintln!("eval: {bin} not found — build `cargo build --release -p modhash-cli` first");
        std::process::exit(2);
    }
    let text = std::fs::read_to_string(&csv).expect("pairs.csv readable");
    let mut pairs: Vec<Pair> = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut c = line.split(',');
        let (Some(a), Some(b), Some(l)) = (c.next(), c.next(), c.next()) else {
            eprintln!("{csv}:{}: expected <a>,<b>,<label>", ln + 1);
            std::process::exit(2);
        };
        match parse_label(l) {
            Some(label) => pairs.push(Pair {
                a: a.trim().to_string(),
                b: b.trim().to_string(),
                label,
            }),
            None => {
                eprintln!("{csv}:{}: bad label `{l}`", ln + 1);
                std::process::exit(2);
            }
        }
    }
    if pairs.is_empty() {
        eprintln!("eval: {csv}: no pairs");
        std::process::exit(2);
    }

    let (mut tp, mut fp, mut fn_, mut tn, mut skipped) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for p in &pairs {
        let out = Command::new(&bin)
            .arg("match")
            .arg("--json")
            .arg(&p.a)
            .arg(&p.b)
            .output()
            .expect("spawn modhash");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Verdict: the calibrated threshold when given (distance <= N),
        // else the advisory `matched` flag.
        let predicted = match threshold {
            Some(t) => match json_distance(&stdout) {
                Some(d) => d <= t,
                None => {
                    skipped += 1;
                    continue;
                }
            },
            None => match json_matched(&stdout) {
                Some(m) => m,
                None => {
                    skipped += 1;
                    continue;
                }
            },
        };
        match (predicted, p.label) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, true) => fn_ += 1,
            (false, false) => tn += 1,
        }
    }
    let denom_p = tp + fp;
    let denom_r = tp + fn_;
    let precision = if denom_p == 0 { 0.0 } else { tp as f64 / denom_p as f64 };
    let recall = if denom_r == 0 { 0.0 } else { tp as f64 / denom_r as f64 };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    println!("dataset: {csv} ({} pairs)", pairs.len());
    match threshold {
        Some(t) => println!("verdict: distance <= {t}"),
        None => println!("verdict: advisory bound (per-modality)"),
    }
    println!("tp={tp} fp={fp} fn={fn_} tn={tn} skipped={skipped}");
    println!("precision: {precision:.6}");
    println!("recall:    {recall:.6}");
    println!("f1:        {f1:.6}");
}
