//! Conformance: decode committed fixtures byte-exact against the
//! reference decoder.
//!
//! PROVENANCE: `*.jpg` encoded and `*.raw` decoded offline with Pillow
//! 12.3.0 (libjpeg-turbo backend — ISLOW integer IDCT, fancy upsampling)
//! except `base_440.jpg` which was encoded by ffmpeg 9.0.1
//! (`-pix_fmt yuvj440p`) and decoded by Pillow. `.raw` files are
//! `Image.tobytes()` output. Generator: `tests/fixtures/gen_fixtures.py`.
//! See `tests/fixtures/PROVENANCE.md`.
//!
//! Tolerance policy: none. Every expected byte must match exactly —
//! the decode pipeline (islow IDCT + triangle upsampling + fixed-point
//! YCbCr) replicates libjpeg's integer arithmetic bit-for-bit.

use modhash_jpeg::{Jpeg, decode};

fn expect(name: &str) -> (Vec<u8>, Vec<u8>) {
    let jpg = std::fs::read(format!("tests/fixtures/{name}.jpg")).unwrap();
    let raw = std::fs::read(format!("tests/fixtures/{name}.raw")).unwrap();
    (jpg, raw)
}

fn check(name: &str, w: u32, h: u32) -> Vec<u8> {
    let (jpg, raw) = expect(name);
    let img = decode(&jpg).unwrap_or_else(|e| panic!("{name}: decode failed: {e:?}"));
    let got = match &img {
        Jpeg::Gray(i) => i.as_slice().to_vec(),
        Jpeg::Rgb(i) => i.as_slice().to_vec(),
    };
    assert_eq!(img.width(), w, "{name} width");
    assert_eq!(img.height(), h, "{name} height");
    assert_eq!(got.len(), raw.len(), "{name} raw length");
    if got != raw {
        let mut ndiff = 0;
        let mut maxd = 0i32;
        let mut first = None;
        for (i, (a, b)) in got.iter().zip(raw.iter()).enumerate() {
            let d = (*a as i32 - *b as i32).abs();
            if d > 0 {
                ndiff += 1;
                if first.is_none() {
                    first = Some((i, *a, *b));
                }
                maxd = maxd.max(d);
            }
        }
        panic!(
            "{name}: {ndiff}/{} samples differ, max |Δ| = {maxd}, first at {first:?}",
            got.len()
        );
    }
    jpg
}

// ---- Baseline (SOF0) ----

#[test]
fn baseline_444() {
    check("base_444", 32, 24);
}

#[test]
fn baseline_422() {
    check("base_422", 40, 24);
}

/// 4:2:0 with DRI=2 restart markers.
#[test]
fn baseline_420_restart() {
    check("base_420_rst", 48, 32);
}

/// 4:2:2 at odd width: the triangle filter emits 2*ceil(W/2) = W+1
/// samples per row; the crate must crop, not overrun.
#[test]
fn baseline_422_odd_width() {
    check("base_422_odd", 41, 23);
}

/// 4:2:0 at odd width AND height.
#[test]
fn baseline_420_odd() {
    check("base_420_odd", 33, 17);
}

/// 4:4:0 (vertical-only subsampling) via ffmpeg.
#[test]
fn baseline_440() {
    check("base_440", 24, 32);
}

#[test]
fn baseline_gray() {
    check("base_gray", 24, 40);
}

// ---- Progressive (SOF2) ----

#[test]
fn progressive_420() {
    check("prog_420", 32, 24);
}

#[test]
fn progressive_444() {
    check("prog_444", 40, 24);
}

#[test]
fn progressive_gray() {
    check("prog_gray", 24, 40);
}

/// Progressive with restart markers (DRI=2 between scan data).
#[test]
fn progressive_444_restart() {
    check("prog_444_rst", 32, 24);
}

// ---- Truncation / corruption ----

/// Every strict prefix must decode or error — never panic.
#[test]
fn prefix_scan_never_panics() {
    for name in [
        "base_444",
        "base_420_rst",
        "base_gray",
        "prog_420",
        "prog_444_rst",
    ] {
        let (jpg, _) = expect(name);
        for i in 0..jpg.len() {
            let _ = decode(&jpg[..i]); // panic = test failure
        }
    }
}

/// Single-byte mutations of real streams: 20k+ corruptions, no panics.
/// Deterministic counter walk over (position, xor) space.
#[test]
fn mutation_storm_never_panics() {
    let mut count = 0usize;
    for name in ["base_444", "base_420_rst", "prog_420"] {
        let (jpg, _) = expect(name);
        let n = jpg.len();
        for t in 0..8_000usize {
            let mut m = jpg.clone();
            let pos = (t * 2654435761usize.wrapping_add(0x9e3779b9)) % n;
            let xor = ((t >> 4) as u8) | 1;
            m[pos] ^= xor;
            let _ = decode(&m);
            count += 1;
        }
    }
    assert!(count >= 20_000);
}

// ---- Explicit Unsupported / error paths ----

/// Arithmetic-coded SOF must name the coding process.
#[test]
fn arithmetic_frame_is_unsupported() {
    let (jpg, _) = expect("base_444");
    // SOF0 marker FFC0 → FFC9 (extended sequential, arithmetic).
    let i = jpg.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
    let mut m = jpg.clone();
    m[i + 1] = 0xc9;
    let e = decode(&m).unwrap_err();
    let s = format!("{e}");
    assert!(s.contains("arithmetic"), "got {s}");
}

/// Lossless SOF3 must name the coding process.
#[test]
fn lossless_frame_is_unsupported() {
    let (jpg, _) = expect("base_444");
    let i = jpg.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
    let mut m = jpg.clone();
    m[i + 1] = 0xc3;
    let e = decode(&m).unwrap_err();
    assert!(format!("{e}").contains("lossless"));
}

#[test]
fn garbage_is_rejected() {
    assert!(decode(&[]).is_err());
    assert!(decode(&[0xff]).is_err());
    assert!(decode(b"not a jpeg at all").is_err());
    // SOI then EOF.
    assert!(decode(&[0xff, 0xd8]).is_err());
}
