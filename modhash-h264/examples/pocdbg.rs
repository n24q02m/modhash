//! Debug: per-fixture decode probe.
use std::fs;

fn main() {
    for name in std::env::args().skip(1) {
        let stream = fs::read(format!("tests/fixtures/{name}.h264")).unwrap();
        match modhash_h264::decode(&stream) {
            Ok(frames) => println!("{name}: OK {} frames", frames.len()),
            Err(e) => println!("{name}: ERR {e:?}"),
        }
    }
}
