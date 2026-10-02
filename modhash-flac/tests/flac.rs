//! Conformance tests for `modhash-flac`.
//!
//! There is no encoder in the workspace, so the harness below is a
//! test-local FLAC writer: every frame in these tests is assembled
//! bit-by-bit (including the CRC-8 and CRC-16) exactly as RFC 9639
//! describes, and the decoder is checked against samples computed by
//! hand in the test, not against anything the decoder produced.

use modhash_flac::{Flac, Limits, decode, decode_streaminfo};
use modhash_primitives::Error;

// ------------------------------------------------------------------
// Test-local encoder
// ------------------------------------------------------------------

/// MSB-first bit writer: bit order is bit 7 of each byte first, matching
/// the stream order FLAC uses for every field.
struct BitW {
    bytes: Vec<u8>,
    bit: u32,
}

impl BitW {
    fn new() -> Self {
        BitW {
            bytes: Vec::new(),
            bit: 0,
        }
    }

    fn bits(&mut self, v: u64, n: u32) {
        assert!(n <= 64);
        assert!(
            n == 64 || v < (1u64 << n),
            "value {v} does not fit in {n} bits"
        );
        for i in (0..n).rev() {
            self.bit(((v >> i) & 1) != 0);
        }
    }

    fn bit(&mut self, b: bool) {
        if self.bit == 0 {
            self.bytes.push(0);
        }
        if b {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> self.bit;
        }
        self.bit = (self.bit + 1) % 8;
    }

    /// Signed two's-complement field of `n` bits.
    fn signed(&mut self, v: i64, n: u32) {
        if n == 0 {
            assert_eq!(v, 0, "a 0-bit field can only encode 0");
            return;
        }
        assert!(
            v >= -(1i64 << (n - 1)) && v < (1i64 << (n - 1)),
            "signed value {v} does not fit in {n} bits"
        );
        let masked = (v as u64) & ((1u64 << n) - 1);
        self.bits(masked, n);
    }

    /// FLAC unary: `q` zero bits then a one bit.
    fn unary(&mut self, q: u32) {
        for _ in 0..q {
            self.bit(false);
        }
        self.bit(true);
    }

    /// Pads with zero bits to the next byte boundary.
    fn pad(&mut self) {
        while self.bit != 0 {
            self.bit(false);
        }
    }

    /// Whole bytes written so far; only valid byte-aligned.
    fn bytes(&self) -> Vec<u8> {
        assert_eq!(self.bit, 0, "byte-aligned read of BitW");
        self.bytes.clone()
    }
}

/// CRC-8 as FLAC uses it (poly 0x07, init 0, MSB-first). Duplicated in
/// the test rather than imported: the decoder's copies are private and
/// the test computing its own CRC is what makes a CRC test a real check.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16 as FLAC uses it (poly 0x8005, init 0, MSB-first).
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A STREAMINFO block. `last` is the last-metadata-block flag.
fn streaminfo(rate: u32, channels: u8, bps: u8, total: u64, last: bool) -> Vec<u8> {
    let mut w = BitW::new();
    // Block header: last flag, type 0, length 34.
    w.bits(u64::from(last), 1);
    w.bits(0, 7);
    w.bits(34, 24);
    // Body (RFC 9639 section 8.2).
    w.bits(192, 16); // min block size
    w.bits(4608, 16); // max block size
    w.bits(0, 24); // min frame size: unknown
    w.bits(0, 24); // max frame size: unknown
    w.bits(u64::from(rate), 20);
    w.bits(u64::from(channels - 1), 3);
    w.bits(u64::from(bps - 1), 5);
    w.bits(total, 36);
    for _ in 0..16 {
        w.bits(0, 8); // md5
    }
    w.bytes()
}

/// A skipped (non-STREAMINFO) metadata block of `len` zero bytes.
fn skip_block(ty: u8, len: u32, last: bool) -> Vec<u8> {
    let mut w = BitW::new();
    w.bits(u64::from(last), 1);
    w.bits(u64::from(ty), 7);
    w.bits(u64::from(len), 24);
    for _ in 0..len {
        w.bits(0, 8);
    }
    w.bytes()
}

/// The UTF-8-style coded frame/sample number, RFC 9639 section 9.2.1.
fn coded(w: &mut BitW, v: u64) {
    if v < 0x80 {
        w.bits(v, 8);
        return;
    }
    // Smallest byte count whose capacity fits v: `n` bytes carry the
    // lead's (7-n) payload bits plus (n-1) continuation groups of 6.
    let nbytes = (2..=7u32)
        .find(|&nb| v < (1u64 << (7 - nb + 6 * (nb - 1))))
        .expect("coded number fits in 7 bytes");
    let lead_payload = 7 - nbytes;
    let lead = (0xFFu8 << (8 - nbytes))
        | (((v >> (6 * (nbytes - 1))) as u8) & ((1u8 << lead_payload) - 1));
    w.bits(u64::from(lead), 8);
    for i in (0..nbytes - 1).rev() {
        w.bits(0x80 | ((v >> (6 * i)) & 0x3F), 8);
    }
}

/// One subframe of a frame being built.
enum Sub {
    /// Type 000000: one signed value repeated `block_size` times.
    Constant { value: i64, wasted: u32 },
    /// Type 000001: every sample verbatim.
    Verbatim { samples: Vec<i64>, wasted: u32 },
    /// Type 001ooo: warmup samples plus a Rice-coded residual.
    Fixed {
        order: u32,
        warmup: Vec<i64>,
        residual: Rice,
    },
}

/// A Rice residual block, with per-partition parameters chosen
/// explicitly so the test pins the read order rather than whatever an
/// optimal encoder would pick.
struct Rice {
    /// 0 for 4-bit parameters, 1 for 5-bit.
    method: u32,
    /// Partition order.
    porder: u32,
    /// One entry per partition (`1 << porder` entries).
    parts: Vec<Part>,
    /// The residuals, flat across partitions in stream order.
    data: Vec<i64>,
}

/// One Rice partition.
enum Part {
    /// Normal Rice coding at the given parameter.
    Rice(u32),
    /// Escape: raw `width`-bit signed residuals.
    Escape(u32),
}

/// Zig-zag encode: the FLAC residual map (even = non-negative).
fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn write_subframe(w: &mut BitW, sub: &Sub, block_size: u32, eff_bps: u32) {
    w.bit(false); // pad bit
    match sub {
        Sub::Constant { .. } => w.bits(0, 6),
        Sub::Verbatim { .. } => w.bits(1, 6),
        Sub::Fixed { order, .. } => w.bits(u64::from(8 + order), 6),
    }
    let wasted = match sub {
        Sub::Constant { wasted, .. } | Sub::Verbatim { wasted, .. } => *wasted,
        Sub::Fixed { .. } => 0,
    };
    w.bit(wasted > 0);
    if wasted > 0 {
        w.unary(wasted - 1);
    }
    let width = eff_bps - wasted;
    match sub {
        Sub::Constant { value, .. } => w.signed(value >> wasted, width),
        Sub::Verbatim { samples, .. } => {
            assert_eq!(samples.len() as u32, block_size);
            for s in samples {
                w.signed(s >> wasted, width);
            }
        }
        Sub::Fixed {
            order,
            warmup,
            residual,
        } => {
            assert_eq!(warmup.len(), *order as usize);
            for &s in warmup {
                w.signed(s, eff_bps);
            }
            write_rice(w, residual, block_size, *order);
        }
    }
}

fn write_rice(w: &mut BitW, r: &Rice, block_size: u32, order: u32) {
    let parts = 1u32 << r.porder;
    assert_eq!(r.parts.len(), parts as usize);
    assert_eq!(block_size % parts, 0, "partitions must divide the block");
    w.bits(u64::from(r.method), 2);
    w.bits(u64::from(r.porder), 4);
    let pwidth = if r.method == 0 { 4 } else { 5 };
    let part_len = block_size >> r.porder;
    let mut pos = 0usize;
    for (p, part) in r.parts.iter().enumerate() {
        // The decoder's split: partition 0 codes `order` fewer residuals.
        let count = (if p == 0 { part_len - order } else { part_len }) as usize;
        let chunk = &r.data[pos..pos + count];
        pos += count;
        match *part {
            Part::Rice(param) => {
                w.bits(u64::from(param), pwidth);
                for &v in chunk {
                    let u = zigzag(v);
                    w.unary((u >> param) as u32);
                    w.bits(u & ((1u64 << param) - 1), param);
                }
            }
            Part::Escape(width) => {
                w.bits(if r.method == 0 { 15 } else { 31 }, pwidth);
                w.bits(u64::from(width), 5);
                for &v in chunk {
                    w.signed(v, width);
                }
            }
        }
    }
    assert_eq!(pos, r.data.len(), "residual data must be fully consumed");
}

/// Everything a test wants in a frame; the fields map straight onto the
/// FLAC header fields (RFC 9639 section 9.2).
struct FrameSpec {
    /// 4-bit block size code.
    block_code: u32,
    /// Value for the 8/16-bit block-size extra field (the stored
    /// block_size - 1, only used for codes 6/7).
    block_extra: u32,
    /// 4-bit sample rate code.
    rate_code: u32,
    /// Value for the 8/16-bit rate extra field (raw field value).
    rate_extra: u32,
    /// 4-bit channel assignment.
    chan_code: u32,
    /// 3-bit coded sample size; 0 defers to STREAMINFO.
    bps_code: u32,
    /// Coded frame/sample number.
    number: u64,
    /// Resolved block size in samples per channel.
    block_size: u32,
    /// Bits per sample the subframes are coded at, before the side boost.
    bps: u32,
    /// One subframe per channel the map implies.
    subs: Vec<Sub>,
}

impl FrameSpec {
    /// The common case: a small block via code 6 (8-bit extra), rate and
    /// bps deferred to STREAMINFO, one independent mono channel.
    fn mono(block_size: u32, bps: u32, sub: Sub) -> Self {
        FrameSpec {
            block_code: 6,
            block_extra: block_size - 1,
            rate_code: 0,
            rate_extra: 0,
            chan_code: 0,
            bps_code: 0,
            number: 0,
            block_size,
            bps,
            subs: vec![sub],
        }
    }
}

/// Writes one whole frame including both CRCs.
fn frame(spec: &FrameSpec) -> Vec<u8> {
    let mut h = BitW::new();
    h.bits(0x3FFE, 14); // sync
    h.bit(false); // reserved
    h.bit(false); // fixed blocking strategy
    h.bits(u64::from(spec.block_code), 4);
    h.bits(u64::from(spec.rate_code), 4);
    h.bits(u64::from(spec.chan_code), 4);
    h.bits(u64::from(spec.bps_code), 3);
    h.bit(false); // reserved
    coded(&mut h, spec.number);
    match spec.block_code {
        6 => h.bits(u64::from(spec.block_extra), 8),
        7 => h.bits(u64::from(spec.block_extra), 16),
        _ => {}
    }
    match spec.rate_code {
        12 => h.bits(u64::from(spec.rate_extra), 8),
        13 | 14 => h.bits(u64::from(spec.rate_extra), 16),
        _ => {}
    }
    let mut bytes = h.bytes();
    bytes.push(crc8(&bytes));

    // Subframes: the side channel, if any, is coded at bps + 1.
    let side = match spec.chan_code {
        8 | 10 => Some(1usize),
        9 => Some(0usize),
        _ => None,
    };
    let mut s = BitW::new();
    for (i, sub) in spec.subs.iter().enumerate() {
        let boost = u32::from(side == Some(i));
        write_subframe(&mut s, sub, spec.block_size, spec.bps + boost);
    }
    s.pad();
    bytes.extend(s.bytes());
    let crc = crc16(&bytes);
    bytes.push((crc >> 8) as u8);
    bytes.push(crc as u8);
    bytes
}

/// A complete stream: signature, the metadata section (a STREAMINFO
/// block plus whatever blocks `extra_meta` supplies), then frames.
fn stream(si: &[u8], extra_meta: &[u8], frames: &[u8]) -> Vec<u8> {
    let mut v = b"fLaC".to_vec();
    v.extend_from_slice(si);
    v.extend_from_slice(extra_meta);
    v.extend_from_slice(frames);
    v
}

fn dec(input: &[u8]) -> Result<Flac, Error> {
    decode(input, &Limits::default())
}

/// The byte offset where frames start in the one-block streams these
/// tests build: 4-byte signature + 4-byte block header + 34-byte body.
const METADATA_END: usize = 4 + 4 + 34;

/// A small valid stream most tests build on: mono, 16-bit, 44100 Hz,
/// one 8-sample verbatim frame.
fn valid_stream() -> Vec<u8> {
    let si = streaminfo(44_100, 1, 16, 8, true);
    let f = frame(&FrameSpec::mono(
        8,
        16,
        Sub::Verbatim {
            samples: vec![1, -2, 3, -4, 5, -6, 7, -8],
            wasted: 0,
        },
    ));
    stream(&si, &[], &f)
}

/// `samples()` is `&[i32]`; this compares it against an `i64` list.
fn expect_i32(out: &Flac, want: &[i64]) {
    let got: Vec<i64> = out.samples().iter().map(|&v| i64::from(v)).collect();
    assert_eq!(got, want);
}

// ------------------------------------------------------------------
// STREAMINFO
// ------------------------------------------------------------------

#[test]
fn streaminfo_fields_surface() {
    let si = streaminfo(48_000, 2, 24, 123_456_789, true);
    let parsed = decode_streaminfo(&stream(&si, &[], &[])).unwrap();
    assert_eq!(parsed.sample_rate, 48_000);
    assert_eq!(parsed.channels, 2);
    assert_eq!(parsed.bits_per_sample, 24);
    assert_eq!(parsed.total_samples, 123_456_789);
    assert_eq!(parsed.min_block_size, 192);
    assert_eq!(parsed.max_block_size, 4608);
}

#[test]
fn decode_surfaces_streaminfo_and_frame_rate() {
    // STREAMINFO declares 44100; the frame's rate code 0 defers to it.
    let f = dec(&valid_stream()).unwrap();
    assert_eq!(f.sample_rate(), 44_100);
    assert_eq!(f.channels(), 1);
    assert_eq!(f.bits_per_sample(), 16);
    assert_eq!(f.total_samples(), 8);
    assert_eq!(f.frames(), 8);
}

#[test]
fn frame_declared_rate_overrides_empty_streaminfo() {
    // STREAMINFO rate 0 + frame code 9 (9 kHz): the frame carries it.
    let si = streaminfo(0, 1, 16, 8, true);
    let mut spec = FrameSpec::mono(
        8,
        16,
        Sub::Constant {
            value: 0,
            wasted: 0,
        },
    );
    spec.rate_code = 9;
    let f = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    assert_eq!(f.sample_rate(), 9_000);
}

#[test]
fn frame_rate_mismatch_is_rejected() {
    let si = streaminfo(44_100, 1, 16, 16, true);
    let mut spec = FrameSpec::mono(
        8,
        16,
        Sub::Constant {
            value: 0,
            wasted: 0,
        },
    );
    spec.rate_code = 9; // 9 kHz in a 44.1 kHz stream
    match dec(&stream(&si, &[], &frame(&spec))) {
        Err(Error::BadValue(_)) => {}
        other => panic!("expected BadValue, got {other:?}"),
    }
}

// ------------------------------------------------------------------
// Subframes
// ------------------------------------------------------------------

#[test]
fn constant_subframe_decodes() {
    let f = frame(&FrameSpec::mono(
        8,
        16,
        Sub::Constant {
            value: -300,
            wasted: 0,
        },
    ));
    let out = dec(&stream(&streaminfo(44_100, 1, 16, 8, true), &[], &f)).unwrap();
    assert_eq!(out.samples(), &[-300; 8]);
}

#[test]
fn verbatim_subframe_decodes() {
    let samples = vec![0i64, 1, -1, 32767, -32768, 1234, -4321, 42];
    let f = frame(&FrameSpec::mono(
        8,
        16,
        Sub::Verbatim {
            samples: samples.clone(),
            wasted: 0,
        },
    ));
    let out = dec(&stream(&streaminfo(44_100, 1, 16, 8, true), &[], &f)).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn verbatim_24bit_sign_extends() {
    // -2^23 and +2^23-1 exercise the sign edge of the widest common width.
    let samples = vec![-8_388_608i64, 8_388_607];
    let mut spec = FrameSpec::mono(
        2,
        24,
        Sub::Verbatim {
            samples: samples.clone(),
            wasted: 0,
        },
    );
    spec.bps_code = 6; // 24-bit declared in the header too
    let si = streaminfo(44_100, 1, 24, 2, true);
    let out = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    expect_i32(&out, &samples);
    assert_eq!(out.bits_per_sample(), 24);
}

#[test]
fn wasted_bits_shift_samples_up() {
    // Samples coded with 4 wasted bits: the stored value is s >> 4 at
    // bps - 4 bits, decoded value is the stored value << 4.
    let f = frame(&FrameSpec::mono(
        4,
        16,
        Sub::Constant {
            value: 0x1230, // ends in 4 zero bits
            wasted: 4,
        },
    ));
    let out = dec(&stream(&streaminfo(44_100, 1, 16, 4, true), &[], &f)).unwrap();
    assert_eq!(out.samples(), &[0x1230; 4]);
}

// ------------------------------------------------------------------
// Fixed predictors, orders 0-4, Rice-coded residuals
// ------------------------------------------------------------------

/// Computes the warmup and residual pair for a fixed predictor of
/// `order` over `samples`, the way the format defines it: residuals are
/// exactly what the decoder's recurrences invert.
fn encode_fixed(order: u32, samples: &[i64]) -> (Vec<i64>, Vec<i64>) {
    let warmup = samples[..order as usize].to_vec();
    let mut residual = Vec::with_capacity(samples.len() - order as usize);
    for n in order as usize..samples.len() {
        let s = |k: usize| samples[n - k];
        let predicted = match order {
            0 => 0,
            1 => s(1),
            2 => 2 * s(1) - s(2),
            3 => 3 * s(1) - 3 * s(2) + s(3),
            _ => 4 * s(1) - 6 * s(2) + 4 * s(3) - s(4),
        };
        residual.push(samples[n] - predicted);
    }
    (warmup, residual)
}

/// Assembles a stream of one mono 16-bit fixed-predictor frame.
fn fixed_stream(order: u32, samples: &[i64], rice: Rice) -> Vec<u8> {
    let (warmup, _) = encode_fixed(order, samples);
    let mut spec = FrameSpec::mono(
        samples.len() as u32,
        16,
        Sub::Constant {
            value: 0,
            wasted: 0,
        },
    );
    spec.subs = vec![Sub::Fixed {
        order,
        warmup,
        residual: rice,
    }];
    stream(
        &streaminfo(44_100, 1, 16, samples.len() as u64, true),
        &[],
        &frame(&spec),
    )
}

#[test]
fn fixed_order0_rice4() {
    // Order 0: the residual IS the sample. Residuals
    // [3, -1, 0, 2, -2, 1, 7, -3] zig-zag to [6,1,0,4,3,2,14,5].
    let residual = vec![3i64, -1, 0, 2, -2, 1, 7, -3];
    let samples = residual.clone();
    let data = fixed_stream(
        0,
        &samples,
        Rice {
            method: 0,
            porder: 0,
            parts: vec![Part::Rice(2)],
            data: residual,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn fixed_order1_rice5() {
    // Order 1 on a ramp: warmup 5, residuals all 2 (the delta). Method 1
    // (5-bit parameters) with param 1: each u = 4 -> quotient 2, rem 0.
    let samples: Vec<i64> = (0..8).map(|i| 5 + 2 * i).collect();
    let (_, res) = encode_fixed(1, &samples);
    assert_eq!(res, vec![2; 7]);
    let data = fixed_stream(
        1,
        &samples,
        Rice {
            method: 1,
            porder: 0,
            parts: vec![Part::Rice(1)],
            data: res,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn fixed_order2() {
    // The second difference of n^2 is the constant 2, so order 2 leaves
    // residuals [2,2,2,2,2] plus the tail of a non-square last sample.
    let samples: Vec<i64> = vec![0, 1, 4, 9, 16, 25, 36, 50];
    let (_, res) = encode_fixed(2, &samples);
    assert_eq!(res, vec![2, 2, 2, 2, 2, 3]);
    let data = fixed_stream(
        2,
        &samples,
        Rice {
            method: 0,
            porder: 0,
            parts: vec![Part::Rice(0)],
            data: res,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn fixed_order3() {
    // The third difference of n^3 is the constant 6, so order 3 leaves
    // residuals [6,6,6,6,6] after a 3-sample warmup.
    let samples: Vec<i64> = (0..8).map(|i| i * i * i).collect();
    let (_, res) = encode_fixed(3, &samples);
    assert_eq!(res, vec![6, 6, 6, 6, 6]);
    let data = fixed_stream(
        3,
        &samples,
        Rice {
            method: 0,
            porder: 0,
            parts: vec![Part::Rice(3)],
            data: res,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn fixed_order4() {
    // The fourth difference of n^4 is the constant 24, so order 4 leaves
    // residuals [24,24,24,24] after a 4-sample warmup.
    let samples: Vec<i64> = (0..8).map(|i| i * i * i * i).collect();
    let (_, res) = encode_fixed(4, &samples);
    assert_eq!(res, vec![24, 24, 24, 24]);
    let data = fixed_stream(
        4,
        &samples,
        Rice {
            method: 0,
            porder: 0,
            parts: vec![Part::Rice(3)],
            data: res,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn rice_partitioned_mixed_parameters() {
    // Order 1, block 16, partition order 1: partition 0 codes
    // 16/2 - 1 = 7 residuals, partition 1 codes 8. Deliberately
    // different parameters per partition pin the read order.
    let samples: Vec<i64> = (0..16).map(|i| 100 + 3 * i).collect();
    let (_, res) = encode_fixed(1, &samples);
    assert_eq!(res.len(), 15);
    let data = fixed_stream(
        1,
        &samples,
        Rice {
            method: 0,
            porder: 1,
            parts: vec![Part::Rice(1), Part::Rice(3)],
            data: res,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn rice_escape_partition_decodes_raw() {
    // One escaped partition: a 5-bit raw width, then verbatim residuals.
    let residual = vec![-2i64, -1, 0, 1, 2, 3, -3, 0];
    let samples = residual.clone(); // order 0: residual == sample
    let data = fixed_stream(
        0,
        &samples,
        Rice {
            method: 0,
            porder: 0,
            parts: vec![Part::Escape(5)],
            data: residual,
        },
    );
    let out = dec(&data).unwrap();
    expect_i32(&out, &samples);
}

#[test]
fn lpc_subframe_is_refused_not_decoded() {
    // Type code 32 (LPC order 1) in a frame whose header is otherwise
    // valid: the decoder must refuse by name, not guess.
    let mut h = BitW::new();
    h.bits(0x3FFE, 14);
    h.bit(false);
    h.bit(false);
    h.bits(6, 4); // block code -> 8-bit extra
    h.bits(0, 4); // rate: STREAMINFO
    h.bits(0, 4); // mono
    h.bits(0, 3); // bps: STREAMINFO
    h.bit(false);
    coded(&mut h, 0);
    h.bits(7, 8); // block_size - 1
    let mut bytes = h.bytes();
    bytes.push(crc8(&bytes));
    let mut s = BitW::new();
    s.bit(false);
    s.bits(32, 6); // LPC order 1
    s.bit(false);
    s.signed(0, 16); // warmup
    s.bits(0, 4); // precision - 1
    s.signed(0, 5); // shift
    s.pad();
    bytes.extend(s.bytes());
    let c = crc16(&bytes);
    bytes.push((c >> 8) as u8);
    bytes.push(c as u8);
    let data = stream(&streaminfo(44_100, 1, 16, 8, true), &[], &bytes);
    match dec(&data) {
        Err(Error::Unsupported(_)) => {}
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

// ------------------------------------------------------------------
// Stereo channel assignments
// ------------------------------------------------------------------

fn stereo_spec(chan_code: u32, ch0: Vec<i64>, ch1: Vec<i64>) -> FrameSpec {
    let n = ch0.len() as u32;
    FrameSpec {
        chan_code,
        block_size: n,
        subs: vec![
            Sub::Verbatim {
                samples: ch0,
                wasted: 0,
            },
            Sub::Verbatim {
                samples: ch1,
                wasted: 0,
            },
        ],
        ..FrameSpec::mono(
            n,
            16,
            Sub::Constant {
                value: 0,
                wasted: 0,
            },
        )
    }
}

fn expect_stereo(out: &Flac, left: &[i64], right: &[i64]) {
    let mut want = Vec::new();
    for i in 0..left.len() {
        want.push(left[i]);
        want.push(right[i]);
    }
    expect_i32(out, &want);
}

#[test]
fn stereo_left_side_decorrelates() {
    // left/side: ch0 = left at bps, ch1 = left-right at bps+1.
    let left = [100i64, -50, 0, 7];
    let right = [90i64, -60, 5, 7];
    let side: Vec<i64> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
    let spec = stereo_spec(8, left.to_vec(), side);
    let si = streaminfo(44_100, 2, 16, 4, true);
    let out = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    expect_stereo(&out, &left, &right);
}

#[test]
fn stereo_right_side_decorrelates() {
    // right/side: ch0 = left-right at bps+1, ch1 = right at bps.
    let left = [10i64, -20, 30, -40];
    let right = [4i64, -5, 6, -7];
    let side: Vec<i64> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
    let spec = stereo_spec(9, side, right.to_vec());
    let si = streaminfo(44_100, 2, 16, 4, true);
    let out = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    expect_stereo(&out, &left, &right);
}

#[test]
fn stereo_mid_side_decorrelates_odd_side() {
    // mid/side where l + r is odd, exercising the `| s & 1` floor:
    // l=5, r=2 gives mid = 3, side = 3 (odd).
    let left = [5i64, -8, 0, 100];
    let right = [2i64, -4, 1, -50];
    let mid: Vec<i64> = left.iter().zip(&right).map(|(l, r)| (l + r) >> 1).collect();
    let side: Vec<i64> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
    assert_eq!(side[0] & 1, 1, "test requires an odd side value");
    let spec = stereo_spec(10, mid, side);
    let si = streaminfo(44_100, 2, 16, 4, true);
    let out = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    expect_stereo(&out, &left, &right);
}

// ------------------------------------------------------------------
// Multi-frame and header variants
// ------------------------------------------------------------------

#[test]
fn two_frames_concatenate() {
    let f0 = frame(&FrameSpec::mono(
        8,
        16,
        Sub::Verbatim {
            samples: (0..8).collect(),
            wasted: 0,
        },
    ));
    let mut spec1 = FrameSpec::mono(
        8,
        16,
        Sub::Verbatim {
            samples: (8..16).collect(),
            wasted: 0,
        },
    );
    spec1.number = 1;
    let si = streaminfo(44_100, 1, 16, 16, true);
    let out = dec(&stream(&si, &[], &[f0, frame(&spec1)].concat())).unwrap();
    expect_i32(&out, &(0..16).collect::<Vec<i64>>());
    assert_eq!(out.frames(), 16);
}

#[test]
fn multibyte_coded_number_decodes() {
    // Frame number 300 needs the 2-byte coded form (0xC4 0xAC).
    let mut spec = FrameSpec::mono(
        8,
        16,
        Sub::Constant {
            value: 7,
            wasted: 0,
        },
    );
    spec.number = 300;
    let si = streaminfo(44_100, 1, 16, 8, true);
    let out = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    assert_eq!(out.samples(), &[7; 8]);
}

#[test]
fn block_code_192_and_extra_codes_decode() {
    // Code 1 => fixed 192-sample block, no extra field.
    let mut spec = FrameSpec::mono(
        192,
        16,
        Sub::Constant {
            value: -5,
            wasted: 0,
        },
    );
    spec.block_code = 1;
    let si = streaminfo(44_100, 1, 16, 192, true);
    let out = dec(&stream(&si, &[], &frame(&spec))).unwrap();
    assert_eq!(out.samples().len(), 192);

    // Code 7 => 16-bit extra field (block 300 stored as 299).
    let mut spec2 = FrameSpec::mono(
        300,
        16,
        Sub::Constant {
            value: 11,
            wasted: 0,
        },
    );
    spec2.block_code = 7;
    let si2 = streaminfo(44_100, 1, 16, 300, true);
    let out2 = dec(&stream(&si2, &[], &frame(&spec2))).unwrap();
    assert_eq!(out2.samples(), &[11; 300]);
}

#[test]
fn metadata_blocks_after_streaminfo_are_skipped() {
    // A VORBIS_COMMENT-shaped block (type 4) between STREAMINFO and the
    // frames: skipped by length, never interpreted.
    let si = streaminfo(44_100, 1, 16, 8, false);
    let sk = skip_block(4, 12, true);
    let f = frame(&FrameSpec::mono(
        8,
        16,
        Sub::Constant {
            value: 42,
            wasted: 0,
        },
    ));
    let out = dec(&stream(&si, &sk, &f)).unwrap();
    assert_eq!(out.samples(), &[42; 8]);
}

// ------------------------------------------------------------------
// Errors: named, never panic
// ------------------------------------------------------------------

#[test]
fn bad_signature_is_invalid_magic() {
    let mut v = valid_stream();
    v[3] = b'c'; // fLac is not fLaC
    match dec(&v) {
        Err(Error::InvalidMagic { .. }) => {}
        other => panic!("expected InvalidMagic, got {other:?}"),
    }
}

#[test]
fn sync_loss_is_invalid_magic() {
    let mut v = valid_stream();
    assert_eq!(v[METADATA_END], 0xFF, "frame starts with a sync byte");
    v[METADATA_END] = 0x00;
    match dec(&v) {
        Err(Error::InvalidMagic { .. }) => {}
        other => panic!("expected InvalidMagic, got {other:?}"),
    }
}

#[test]
fn bad_frame_crc16_is_bad_value() {
    let mut v = valid_stream();
    let last = v.len() - 1;
    v[last] ^= 0x01; // corrupt the CRC-16 itself
    match dec(&v) {
        Err(Error::BadValue(_)) => {}
        other => panic!("expected BadValue, got {other:?}"),
    }
}

#[test]
fn bad_header_crc8_is_bad_value() {
    let mut v = valid_stream();
    // Flip a bit inside the frame header (the coded-number byte, third
    // byte of the frame): the header CRC-8 must catch it.
    v[METADATA_END + 3] ^= 0x02;
    match dec(&v) {
        Err(Error::BadValue(_)) => {}
        other => panic!("expected BadValue, got {other:?}"),
    }
}

#[test]
fn nonzero_frame_padding_is_rejected() {
    // A 7-bit constant subframe of 4 samples occupies 1+6+1+7 = 15 bits,
    // leaving one pad bit in the last data byte. Set it, fix the CRC-16,
    // and only the zero-pad check can still fire.
    let si = streaminfo(44_100, 1, 7, 4, true);
    let f = frame(&FrameSpec::mono(
        4,
        7,
        Sub::Constant {
            value: 3,
            wasted: 0,
        },
    ));
    let mut v = stream(&si, &[], &f);
    let frame_start = METADATA_END;
    let data_end = v.len() - 2;
    v[data_end - 1] |= 0x01;
    let c = crc16(&v[frame_start..data_end]);
    v[data_end] = (c >> 8) as u8;
    v[data_end + 1] = c as u8;
    match dec(&v) {
        Err(Error::BadValue(_)) => {}
        other => panic!("expected BadValue, got {other:?}"),
    }
}

#[test]
fn every_prefix_fails_without_panic() {
    let v = valid_stream();
    for n in 0..v.len() {
        assert!(
            dec(&v[..n]).is_err(),
            "prefix of {n} bytes must not decode as a complete stream"
        );
    }
}

#[test]
fn bit_flips_never_panic() {
    // Every single-bit flip of a valid stream: must not panic. A flip
    // inside STREAMINFO may legitimately still decode (the block has no
    // checksum of its own); a flip anywhere in a frame always errors
    // because the CRCs cover it.
    let v = valid_stream();
    for i in 0..v.len() * 8 {
        let mut m = v.clone();
        m[i / 8] ^= 1 << (i % 8);
        let _ = dec(&m); // the only assertion: this call returns
    }
}

#[test]
fn frame_bit_flips_always_error() {
    // The CRC-8 and CRC-16 cover every frame byte, so no single-bit flip
    // inside the frame may still decode.
    let v = valid_stream();
    for i in METADATA_END * 8..v.len() * 8 {
        let mut m = v.clone();
        m[i / 8] ^= 1 << (i % 8);
        assert!(dec(&m).is_err(), "flip at bit {i} must not decode");
    }
}

#[test]
fn random_garbage_never_panics() {
    // Deterministic splitmix64 garbage: 512 iterations, lengths 0..128,
    // raw and behind a valid signature; all must return without panic.
    let mut state = 0x2C4A_8E33_7B19_5D00u64;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for _ in 0..512 {
        let len = (next() % 128) as usize;
        let mut v = Vec::with_capacity(len);
        for _ in 0..len {
            v.push(next() as u8);
        }
        let _ = dec(&v);
        let mut s = b"fLaC".to_vec();
        s.extend_from_slice(&v);
        let _ = dec(&s);
    }
}
