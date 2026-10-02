//! Byte-exact hand-built ISO-BMFF fixtures for `modhash-mp4`.
//!
//! Every byte of every fixture is written by the builders below, so the
//! expected sample tables, offsets and `avcC` payloads are known exactly —
//! nothing is round-tripped through the crate under test.

use modhash_mp4::{EntryKind, demux};
use modhash_primitives::{Error, SplitMix64};

/// `size:u32, four, payload`.
fn bx(four: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 8);
    v.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
    v.extend_from_slice(four);
    v.extend_from_slice(payload);
    v
}

/// `size:u32, four, version:u8, flags:u24, payload`.
fn full(four: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(payload.len() + 4);
    p.push(version);
    p.extend_from_slice(&flags.to_be_bytes()[1..]);
    p.extend_from_slice(payload);
    bx(four, &p)
}

fn u16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}
fn u32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}
fn u64(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

/// The identity/unity matrix used by `mvhd`/`tkhd`.
const UNITY_MATRIX: [u8; 36] = [
    0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00,
];

/// `ftyp` with major `isom`, minor 0, compat `isom,avc1,mp42`.
fn ftyp() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(b"isom");
    p.extend_from_slice(&u32(0));
    p.extend_from_slice(b"isom");
    p.extend_from_slice(b"avc1");
    p.extend_from_slice(b"mp42");
    bx(b"ftyp", &p)
}

/// `mvhd` v0: timescale 1000, duration 2000, unity matrix, rate/volume 1.0.
fn mvhd() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(0)); // creation
    p.extend_from_slice(&u32(0)); // modification
    p.extend_from_slice(&u32(1000)); // timescale
    p.extend_from_slice(&u32(2000)); // duration
    p.extend_from_slice(&u32(0x0001_0000)); // rate 1.0
    p.extend_from_slice(&u16(0x0100)); // volume 1.0
    p.extend_from_slice(&u16(0)); // reserved
    p.extend_from_slice(&u64(0)); // reserved
    p.extend_from_slice(&UNITY_MATRIX);
    p.extend_from_slice(&[0; 24]); // pre_defined
    p.extend_from_slice(&u32(2)); // next_track_id
    full(b"mvhd", 0, 0, &p)
}

/// `tkhd` v0 for track `id`, `w`×`h` display size, duration 2000.
fn tkhd(id: u32, w: u16, h: u16) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(0)); // creation
    p.extend_from_slice(&u32(0)); // modification
    p.extend_from_slice(&u32(id));
    p.extend_from_slice(&u32(0)); // reserved
    p.extend_from_slice(&u32(2000)); // duration
    p.extend_from_slice(&u64(0)); // reserved
    p.extend_from_slice(&u16(0)); // layer
    p.extend_from_slice(&u16(0)); // alternate group
    p.extend_from_slice(&u16(0)); // volume
    p.extend_from_slice(&u16(0)); // reserved
    p.extend_from_slice(&UNITY_MATRIX);
    p.extend_from_slice(&u32(u32::from(w) << 16)); // width 16.16
    p.extend_from_slice(&u32(u32::from(h) << 16));
    // flags: enabled | in_movie | in_preview
    full(b"tkhd", 0, 0x0000_0007, &p)
}

/// `mdhd` v0: timescale `ts`, duration `dur`, language `eng` (0x15C7).
fn mdhd(ts: u32, dur: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(0));
    p.extend_from_slice(&u32(0));
    p.extend_from_slice(&u32(ts));
    p.extend_from_slice(&u32(dur));
    // "eng" = 5<<10 | 14<<5 | 7 = 0x15C7; then pre_defined 0.
    p.extend_from_slice(&u16(0x15C7));
    p.extend_from_slice(&u16(0));
    full(b"mdhd", 0, 0, &p)
}

/// `hdlr` for the given handler type.
fn hdlr(kind: &[u8; 4]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(0)); // pre_defined
    p.extend_from_slice(kind);
    p.extend_from_slice(&[0; 12]); // reserved
    p.extend_from_slice(b"handler\0");
    full(b"hdlr", 0, 0, &p)
}

/// The `avcC` payload this suite uses everywhere: one fake SPS and one fake
/// PPS whose bytes are checked verbatim.
fn avcc_payload() -> Vec<u8> {
    let mut p = vec![
        1,    // configurationVersion
        66,   // AVCProfileIndication (baseline)
        0,    // profile_compatibility
        30,   // AVCLevelIndication
        0xFF, // lengthSizeMinusOne = 3 (4-byte NAL lengths)
        0xE1, // numOfSequenceParameterSets = 1
    ];
    p.extend_from_slice(&u16(4));
    p.extend_from_slice(&[0x67, 0x42, 0x00, 0x1E]); // SPS bytes
    p.push(1); // numOfPictureParameterSets
    p.extend_from_slice(&u16(3));
    p.extend_from_slice(&[0x68, 0xCE, 0x06]); // PPS bytes
    p
}

/// `stsd` with one `avc1` visual sample entry (`w`×`h`) carrying `avcC`.
fn stsd_avc1(w: u16, h: u16) -> Vec<u8> {
    let mut entry = Vec::new();
    entry.extend_from_slice(&[0; 6]); // reserved
    entry.extend_from_slice(&u16(1)); // data_reference_index
    entry.extend_from_slice(&u16(0)); // pre_defined
    entry.extend_from_slice(&u16(0)); // reserved
    entry.extend_from_slice(&[0; 12]); // pre_defined[3]
    entry.extend_from_slice(&u16(w));
    entry.extend_from_slice(&u16(h));
    entry.extend_from_slice(&u32(0x0048_0000)); // horizresolution 72dpi
    entry.extend_from_slice(&u32(0x0048_0000)); // vertresolution
    entry.extend_from_slice(&u32(0)); // reserved
    entry.extend_from_slice(&u16(1)); // frame_count
    let mut name = [0u8; 32];
    name[0] = 7;
    name[1..8].copy_from_slice(b"modhash");
    entry.extend_from_slice(&name);
    entry.extend_from_slice(&u16(0x0018)); // depth 24
    entry.extend_from_slice(&u16(0xFFFF)); // pre_defined -1
    entry.extend_from_slice(&bx(b"avcC", &avcc_payload()));
    let mut with_header = Vec::new();
    with_header.extend_from_slice(&u32(entry.len() as u32 + 8));
    with_header.extend_from_slice(b"avc1");
    with_header.extend_from_slice(&entry);
    let mut p = Vec::new();
    p.extend_from_slice(&u32(1)); // entry_count
    p.extend_from_slice(&with_header);
    full(b"stsd", 0, 0, &p)
}

fn stts(runs: &[(u32, u32)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(runs.len() as u32));
    for &(count, delta) in runs {
        p.extend_from_slice(&u32(count));
        p.extend_from_slice(&u32(delta));
    }
    full(b"stts", 0, 0, &p)
}

/// `ctts` v1: one sample per run, `offsets[i]` is sample i's offset.
fn ctts_v1(offsets: &[i32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(offsets.len() as u32));
    for &o in offsets {
        p.extend_from_slice(&u32(1));
        p.extend_from_slice(&u32(o as u32));
    }
    full(b"ctts", 1, 0, &p)
}

fn stsc(runs: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(runs.len() as u32));
    for &(first, per, desc) in runs {
        p.extend_from_slice(&u32(first));
        p.extend_from_slice(&u32(per));
        p.extend_from_slice(&u32(desc));
    }
    full(b"stsc", 0, 0, &p)
}

fn stsz(sizes: &[u32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(0)); // no uniform size
    p.extend_from_slice(&u32(sizes.len() as u32));
    for &s in sizes {
        p.extend_from_slice(&u32(s));
    }
    full(b"stsz", 0, 0, &p)
}

fn stss(indices: &[u32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(indices.len() as u32));
    for &i in indices {
        p.extend_from_slice(&u32(i));
    }
    full(b"stss", 0, 0, &p)
}

fn stco(offsets: &[u64]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(offsets.len() as u32));
    for &o in offsets {
        p.extend_from_slice(&u32(o as u32));
    }
    full(b"stco", 0, 0, &p)
}

fn co64(offsets: &[u64]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u32(offsets.len() as u32));
    for &o in offsets {
        p.extend_from_slice(&u64(o));
    }
    full(b"co64", 0, 0, &p)
}

fn dinf() -> Vec<u8> {
    let url = full(b"url ", 0, 1, &[]); // self-contained
    let mut p = Vec::new();
    p.extend_from_slice(&u32(1));
    p.extend_from_slice(&url);
    let dref = full(b"dref", 0, 0, &p);
    bx(b"dinf", &dref)
}

fn vmhd() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u16(0)); // graphicsmode
    p.extend_from_slice(&[0; 6]); // opcolor
    full(b"vmhd", 0, 1, &p)
}

/// Composition offsets for the minimal file: sample 1 is shown one period
/// late, sample 2 one period early — the classic one-B-frame reorder.
fn ctts_offsets(n: u32) -> Vec<i32> {
    let mut v = vec![0i32; n as usize];
    if n >= 3 {
        v[1] = 45_000;
        v[2] = -45_000;
    }
    v
}

/// `stbl` for a track; `sync` is `None` to omit `stss` entirely.
fn stbl(
    offsets: &[u64],
    wide: bool,
    sizes: &[u32],
    runs: &[(u32, u32, u32)],
    sync: Option<&[u32]>,
) -> Vec<u8> {
    let n_samples = sizes.len() as u32;
    let mut v = Vec::new();
    v.extend_from_slice(&stsd_avc1(640, 360));
    v.extend_from_slice(&stts(&[(n_samples, 45_000)]));
    v.extend_from_slice(&stsc(runs));
    v.extend_from_slice(&stsz(sizes));
    v.extend_from_slice(&if wide { co64(offsets) } else { stco(offsets) });
    if let Some(list) = sync {
        v.extend_from_slice(&stss(list));
    }
    v.extend_from_slice(&ctts_v1(&ctts_offsets(n_samples)));
    bx(b"stbl", &v)
}

fn minf(
    offsets: &[u64],
    wide: bool,
    sizes: &[u32],
    runs: &[(u32, u32, u32)],
    sync: Option<&[u32]>,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&vmhd());
    v.extend_from_slice(&dinf());
    v.extend_from_slice(&stbl(offsets, wide, sizes, runs, sync));
    bx(b"minf", &v)
}

fn mdia(
    offsets: &[u64],
    wide: bool,
    sizes: &[u32],
    runs: &[(u32, u32, u32)],
    sync: Option<&[u32]>,
) -> Vec<u8> {
    let dur = 45_000 * sizes.len() as u32;
    let mut v = Vec::new();
    v.extend_from_slice(&mdhd(90_000, dur));
    v.extend_from_slice(&hdlr(b"vide"));
    v.extend_from_slice(&minf(offsets, wide, sizes, runs, sync));
    bx(b"mdia", &v)
}

fn trak(
    offsets: &[u64],
    wide: bool,
    sizes: &[u32],
    runs: &[(u32, u32, u32)],
    sync: Option<&[u32]>,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&tkhd(1, 640, 360));
    v.extend_from_slice(&mdia(offsets, wide, sizes, runs, sync));
    bx(b"trak", &v)
}

fn moov(
    offsets: &[u64],
    wide: bool,
    sizes: &[u32],
    runs: &[(u32, u32, u32)],
    sync: Option<&[u32]>,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&mvhd());
    v.extend_from_slice(&trak(offsets, wide, sizes, runs, sync));
    bx(b"moov", &v)
}

fn mdat(payload: &[u8]) -> Vec<u8> {
    bx(b"mdat", payload)
}

/// `mdat` with `size == 0`, running to end of file.
fn mdat_to_eof(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&u32(0));
    v.extend_from_slice(b"mdat");
    v.extend_from_slice(payload);
    v
}

/// Assembles the fixture: `ftyp`, optional `free` largesize box, `moov`
/// (built twice — once to measure, once with real chunk offsets), then
/// `mdat`. `gap_bytes` are leading padding *inside* `mdat`'s payload so
/// chunk offsets are nontrivial, and `size0_mdat` switches `mdat` to the
/// to-end-of-file encoding. `sync = None` omits `stss`.
fn build_file(
    wide_offsets: bool,
    largesize_free_box: bool,
    size0_mdat: bool,
    gap_bytes: usize,
    sizes: &[u32],
    runs: &[(u32, u32, u32)],
    sync: Option<&[u32]>,
) -> Vec<u8> {
    let ftyp = ftyp();
    let free = if largesize_free_box {
        largesize_free()
    } else {
        Vec::new()
    };
    // moov's size does not depend on the offsets it contains (stco/co64
    // are fixed-width tables), so a probe with zeros measures where mdat
    // starts; chunk offsets point into mdat's payload after the gap.
    let probe = moov(
        &vec![0; n_chunks(sizes, runs)],
        wide_offsets,
        sizes,
        runs,
        sync,
    );
    let mdat_payload_start = ftyp.len() + free.len() + probe.len() + 8;
    let first_chunk = mdat_payload_start + gap_bytes;
    let offsets = chunk_offsets(first_chunk, sizes, runs);
    let moov = moov(&offsets, wide_offsets, sizes, runs, sync);
    let mut payload = vec![0xEE; gap_bytes];
    payload.extend_from_slice(&concat_samples(sizes));
    let mut file = ftyp;
    file.extend_from_slice(&free);
    file.extend_from_slice(&moov);
    file.extend_from_slice(&if size0_mdat {
        mdat_to_eof(&payload)
    } else {
        mdat(&payload)
    });
    file
}
/// A `free` box in the `size == 1` largesize encoding.
fn largesize_free() -> Vec<u8> {
    let payload = [0xAB; 10];
    let mut v = Vec::new();
    v.extend_from_slice(&u32(1)); // 64-bit size follows
    v.extend_from_slice(b"free");
    v.extend_from_slice(&u64(8 + 8 + payload.len() as u64));
    v.extend_from_slice(&payload);
    v
}

/// Concatenated sample payloads, in sample order: sample `i` is `size`
/// copies of byte `0x40 + i`.
fn concat_samples(sizes: &[u32]) -> Vec<u8> {
    let mut v = Vec::new();
    for (i, &s) in sizes.iter().enumerate() {
        v.extend(std::iter::repeat_n(0x40 + i as u8, s as usize));
    }
    v
}

/// Total chunk count implied by the stsc runs: how many chunks the table
/// needs to hold `sizes.len()` samples. Fixture callers shape runs so the
/// last run fills exactly the needed count.
fn n_chunks(sizes: &[u32], runs: &[(u32, u32, u32)]) -> usize {
    let mut remaining = sizes.len();
    let mut chunk = 0usize;
    for (i, &(first, per, _)) in runs.iter().enumerate() {
        let end = if i + 1 < runs.len() {
            runs[i + 1].0 as usize - 1
        } else {
            usize::MAX
        };
        let mut c = first as usize - 1;
        while c < end && remaining > 0 {
            remaining = remaining.saturating_sub(per as usize);
            c += 1;
            chunk = c;
            if remaining == 0 {
                return chunk;
            }
        }
    }
    chunk
}

/// Absolute file offsets of each chunk, laid out contiguously inside
/// `mdat` starting at `mdat_start` (the payload offset, after the header).
fn chunk_offsets(mdat_start: usize, sizes: &[u32], runs: &[(u32, u32, u32)]) -> Vec<u64> {
    let n = n_chunks(sizes, runs);
    let mut offs = Vec::with_capacity(n);
    let mut at = mdat_start as u64;
    let mut next_sample = 0usize;
    for (i, &(first, per, _)) in runs.iter().enumerate() {
        let end = if i + 1 < runs.len() {
            runs[i + 1].0 as usize - 1
        } else {
            n
        };
        for c in (first as usize - 1)..end.min(n) {
            let _ = c;
            offs.push(at);
            let chunk_bytes: u64 = sizes
                [next_sample..(next_sample + per as usize).min(sizes.len())]
                .iter()
                .map(|&s| u64::from(s))
                .sum();
            at += chunk_bytes;
            next_sample += per as usize;
        }
    }
    while offs.len() < n {
        offs.push(at);
    }
    offs
}

/// `rng.below(n)` as a free helper: SplitMix64 carries `next_u64`/`fill_bytes`
/// only.
fn below(rng: &mut SplitMix64, n: usize) -> usize {
    if n == 0 {
        0
    } else {
        (rng.next_u64() % n as u64) as usize
    }
}

// ---------------------------------------------------------------- tests

/// The canonical small fixture: 4 samples over 2 chunks, sizes [3,5,3,5],
/// one stsc run of 2 samples/chunk, stss [1,3], ctts reorder on samples
/// 1/2.
fn minimal_file() -> Vec<u8> {
    build_file(
        false,
        false,
        false,
        16,
        &[3, 5, 3, 5],
        &[(1, 2, 1)],
        Some(&[1, 3]),
    )
}

#[test]
fn demux_minimal_file() {
    let file = minimal_file();
    let mp4 = demux(&file).expect("minimal fixture must demux");

    assert_eq!(mp4.major_brand, *b"isom");
    assert_eq!(mp4.timescale, 1000);
    assert_eq!(mp4.duration, 2000);
    assert_eq!(mp4.tracks.len(), 1);

    let track = &mp4.tracks[0];
    assert_eq!(track.id, 1);
    assert_eq!(track.timescale, 90_000);
    assert_eq!(track.duration, 180_000);
    assert_eq!(track.language, Some(*b"eng"));
    assert_eq!(track.handler, *b"vide");
    assert_eq!((track.width, track.height), (640, 360));
    assert_eq!(track.table.len(), 4);

    let samples: Vec<_> = track.samples().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(samples.len(), 4);
    // Sizes and offsets: chunk 0 holds samples 0-1, chunk 1 holds 2-3.
    let mdat_payload = file.len() - 16; // 8-byte mdat header + 16 payload bytes
    assert_eq!(samples[0].offset as usize, mdat_payload);
    assert_eq!(samples[0].size, 3);
    assert_eq!(samples[1].offset as usize, mdat_payload + 3);
    assert_eq!(samples[1].size, 5);
    assert_eq!(samples[2].offset as usize, mdat_payload + 8);
    assert_eq!(samples[2].size, 3);
    assert_eq!(samples[3].offset as usize, mdat_payload + 11);
    assert_eq!(samples[3].size, 5);
    // Timing: 45 000 ticks each on a 90 000 Hz timescale.
    assert_eq!(samples[0].decoding, 0);
    assert_eq!(samples[1].decoding, 45_000);
    assert_eq!(samples[2].decoding, 90_000);
    assert_eq!(samples[3].decoding, 135_000);
    assert!(samples.iter().all(|s| s.duration == 45_000));
    // ctts: sample 1 presents +45 000, sample 2 presents -45 000.
    assert_eq!(samples[0].presentation, 0);
    assert_eq!(samples[1].presentation, 90_000);
    assert_eq!(samples[2].presentation, 45_000);
    assert_eq!(samples[3].presentation, 135_000);
    // stss [1,3] (1-based) -> samples 0 and 2 keyframes.
    assert!(samples[0].keyframe);
    assert!(!samples[1].keyframe);
    assert!(samples[2].keyframe);
    assert!(!samples[3].keyframe);
    // Payload bytes round-trip through the recorded ranges.
    assert_eq!(track.sample_bytes(&file, 0).unwrap(), &[0x40; 3]);
    assert_eq!(track.sample_bytes(&file, 1).unwrap(), &[0x41; 5]);
    assert_eq!(track.sample_bytes(&file, 3).unwrap(), &[0x43; 5]);
    assert!(track.sample_bytes(&file, 4).is_err());
    // The file's video track is findable through the convenience accessor.
    assert!(mp4.video_track().is_some());
}

#[test]
fn co64_variant_gives_same_sample_map() {
    let file = build_file(
        true,
        false,
        false,
        16,
        &[3, 5, 3, 5],
        &[(1, 2, 1)],
        Some(&[1, 3]),
    );
    let mp4 = demux(&file).expect("co64 fixture must demux");
    let samples: Vec<_> = mp4.tracks[0]
        .samples()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mdat_payload = file.len() - 16;
    let expected = [
        mdat_payload,
        mdat_payload + 3,
        mdat_payload + 8,
        mdat_payload + 11,
    ];
    for (s, &off) in samples.iter().zip(&expected) {
        assert_eq!(s.offset as usize, off);
    }
}

#[test]
fn largesize_and_size0_boxes() {
    // `free` uses size==1 (largesize) before moov; `mdat` uses size==0
    // (to end of file) at the tail.
    let file = build_file(
        false,
        true,
        true,
        8,
        &[3, 5, 3, 5],
        &[(1, 2, 1)],
        Some(&[1, 3]),
    );
    let mp4 = demux(&file).expect("largesize/size0 fixture must demux");
    let samples: Vec<_> = mp4.tracks[0]
        .samples()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples.len(), 4);
    // mdat is the last box with size 0: its payload runs to EOF, so the
    // final sample must end exactly at file end.
    assert_eq!(samples[3].offset as usize + 5, file.len());
    assert_eq!(samples[0].size, 3);
}

#[test]
fn stsc_multi_run_expansion() {
    // Three runs: chunks 1-2 get 3 samples each, chunk 3 gets 1, chunks
    // 4-5 get 2 each. Sizes are all 4 bytes so the offset arithmetic is
    // checked purely by position: boundaries at samples [0,3), [3,6),
    // [6,7), [7,9), [9,10) → chunk starts 0,12,24,28,36.
    let sizes = [4u32; 10];
    let runs = [(1, 3, 1), (3, 1, 1), (4, 2, 1)];
    let file = build_file(false, false, false, 0, &sizes, &runs, Some(&[1]));
    let mp4 = demux(&file).expect("multi-run fixture must demux");
    let track = &mp4.tracks[0];
    let samples: Vec<_> = track.samples().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(samples.len(), 10);
    let base = file.len() - 40;
    let expected = [0, 4, 8, 12, 16, 20, 24, 28, 32, 36];
    for (i, s) in samples.iter().enumerate() {
        assert_eq!(s.offset as usize, base + expected[i], "sample {i}");
        assert_eq!(s.size, 4);
    }
    // With stss [1] only sample 0 is sync.
    assert!(samples[0].keyframe);
    assert!(samples[1..].iter().all(|s| !s.keyframe));
}

#[test]
fn truncated_file_errors_not_panics() {
    let file = minimal_file();
    // Every strict prefix must produce Err and must never panic. mdat
    // carries an explicit size in this fixture, so a truncated tail errors
    // rather than completing a size-0-to-EOF box.
    for n in 0..file.len() {
        let result = std::panic::catch_unwind(|| demux(&file[..n]));
        assert!(result.is_ok(), "demux panicked on prefix len {n}");
        assert!(
            result.unwrap().is_err(),
            "truncated file parsed as valid at len {n}"
        );
    }
    // The full file is the only good length.
    assert!(demux(&file).is_ok());
}

#[test]
fn mutated_bytes_never_panic() {
    // SplitMix64-driven deterministic corruption: bit flips, drops,
    // splices and inserts over the valid fixture must never panic the
    // demuxer.
    let file = minimal_file();
    let mut rng = SplitMix64::new(0xdead_beef);
    for _ in 0..20_000 {
        let mut corrupt = file.clone();
        match rng.next_u64() % 4 {
            0 => {
                for _ in 0..1 + below(&mut rng, 8) {
                    let i = below(&mut rng, corrupt.len());
                    corrupt[i] ^= 1 << (rng.next_u64() % 8);
                }
            }
            1 => corrupt.truncate(below(&mut rng, corrupt.len() + 1)),
            2 => {
                if !corrupt.is_empty() {
                    let a = below(&mut rng, corrupt.len());
                    let b = below(&mut rng, corrupt.len());
                    let (lo, hi) = (a.min(b), a.max(b));
                    corrupt.drain(lo..hi.max(lo + 1).min(corrupt.len()));
                }
            }
            _ => {
                let at = below(&mut rng, corrupt.len() + 1);
                let v = rng.next_u64() as u8;
                for _ in 0..below(&mut rng, 16) {
                    corrupt.insert(at.min(corrupt.len()), v);
                }
            }
        }
        let r = std::panic::catch_unwind(|| demux(&corrupt));
        assert!(r.is_ok(), "demux panicked on mutated input");
    }
}

#[test]
fn avcc_record_extracted_verbatim() {
    let file = minimal_file();
    let mp4 = demux(&file).unwrap();
    let track = &mp4.tracks[0];
    assert_eq!(track.avcc().unwrap(), &avcc_payload()[..]);
    // And via the raw entry kind, for the consumer that wants the shape.
    match &track.table.description.entries[0] {
        EntryKind::Visual {
            coding,
            width,
            height,
            avcc,
            ..
        } => {
            assert_eq!(coding, b"avc1");
            assert_eq!((*width, *height), (640, 360));
            assert_eq!(avcc.as_deref().unwrap(), &avcc_payload()[..]);
        }
        other => panic!("expected Visual entry, got {other:?}"),
    }
}

#[test]
fn missing_ftyp_is_invalid_magic() {
    // A valid moov without an ftyp prefix is not an MP4 file.
    let file = minimal_file();
    let ftyp_len = ftyp().len();
    let no_ftyp = &file[ftyp_len..];
    let err = demux(no_ftyp).unwrap_err();
    assert!(
        matches!(err, Error::InvalidMagic { .. }),
        "expected InvalidMagic, got {err:?}"
    );
    // An ftyp naming only foreign brands is also not ours.
    let mut weird = Vec::new();
    weird.extend_from_slice(b"xxxx");
    weird.extend_from_slice(&u32(0));
    weird.extend_from_slice(b"yyyy");
    let mut bad = bx(b"ftyp", &weird);
    bad.extend_from_slice(&file[ftyp_len..]);
    let err = demux(&bad).unwrap_err();
    assert!(
        matches!(err, Error::InvalidMagic { .. }),
        "expected InvalidMagic, got {err:?}"
    );
}

#[test]
fn fragmented_file_is_unsupported() {
    let mut file = minimal_file();
    // One empty moof box at top level is enough: the format is fragmented
    // the moment moof appears, whatever else the file contains.
    file.extend_from_slice(&bx(b"moof", &[]));
    let err = demux(&file).unwrap_err();
    assert!(
        matches!(err, Error::Unsupported(_)),
        "expected Unsupported, got {err:?}"
    );
}

#[test]
fn no_stss_means_every_sample_sync() {
    // ISO/IEC 14496-12 §8.6.2: absence of stss marks every sample sync —
    // the spec's metamorphic rule that dropping stss must not change the
    // frame enumeration.
    let file = build_file(false, false, false, 16, &[3, 5, 3, 5], &[(1, 2, 1)], None);
    let mp4 = demux(&file).expect("no-stss fixture must demux");
    let samples: Vec<_> = mp4.tracks[0]
        .samples()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples.len(), 4);
    assert!(
        samples.iter().all(|s| s.keyframe),
        "absent stss must mark every sample sync"
    );
}
