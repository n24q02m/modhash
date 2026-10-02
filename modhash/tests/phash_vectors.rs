//! GENERATED FILE - do not edit.
//!
//! Written by `lab/phash_oracle.py` (the independent pHash oracle).
//! Each row: (fixture file under tests/fixtures/, expected pHash,
//! width, height). The expected value is computed by the Python
//! implementation of docs/algorithms/kit.md section 3 - the Rust
//! pipeline must match it exactly.

/// (fixture name, expected 64-bit pHash, width, height).
#[allow(dead_code)] // also compiles standalone as a test target
pub(crate) const PHASH_VECTORS: &[(&str, u64, u32, u32)] = &[
    ("phash_rgb8_48x40.png", 0xc21b869527e1a74f, 48, 40),
    ("phash_rgba8_48x40.png", 0x84334f9d2c5af4a9, 48, 40),
    ("phash_gray8_40x32.png", 0xd3c706284777fc81, 40, 32),
    ("phash_rgb16_40x24.png", 0xa82d435d3c7d7059, 40, 24),
    ("phash_gray16_36x28.png", 0xd38604056df8c37f, 36, 28),
    ("phash_pal8_trns_52x44.png", 0xf260e731eb4a3ce0, 52, 44),
    ("phash_adam7_37x29.png", 0x8b4bd90bf426b4f0, 37, 29),
    ("phash_small_13x9.png", 0xaa27a317f85ca316, 13, 9),
    ("base_444.png", 0x8d0a3f411ee50f7a, 32, 24),
];
