//! Dump decoded frames as yuv420p rawvideo for ffmpeg diff.
use std::fs;
use std::io::Write;
fn main() {
    for name in std::env::args().skip(1) {
        let stream = fs::read(format!("tests/fixtures/{name}.h264")).unwrap();
        match modhash_h264::decode(&stream) {
            Ok(frames) => {
                let mut out = Vec::new();
                for f in &frames {
                    out.extend_from_slice(&f.y);
                    out.extend_from_slice(&f.cb);
                    out.extend_from_slice(&f.cr);
                }
                fs::File::create(format!("tests/fixtures/{name}.mine.yuv"))
                    .unwrap()
                    .write_all(&out)
                    .unwrap();
                println!("{name}: OK {} frames -> .mine.yuv", frames.len());
            }
            Err(e) => println!("{name}: ERR {e:?}"),
        }
    }
}
