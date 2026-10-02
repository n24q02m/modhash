//! Video fingerprints: mp4 demux → H.264 decode → fixed-rate frame
//! sampling → per-frame pHash chain → MinHash (spec §4.2).
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in
//! this workspace builds without a single registry package.
//!
//! # Pipeline
//!
//! 1. [`modhash_mp4::demux`] resolves the sample tables; the first
//!    `vide` track is the video lane (ISO/IEC 14496-12).
//! 2. The track's `stsd` must carry an `avc1`/`avc3` entry with an
//!    `avcC` record; every other coding (HEVC `hev1`/`hvc1`, MPEG-4
//!    `mp4v`, VP9/AV1, audio-only files) is [`Error::Unsupported`]
//!    naming what was refused.
//! 3. Samples become one Annex-B stream for [`modhash_h264`]: `avcC`'s
//!    SPS/PPS first, then each sample's length-prefixed NAL units with
//!    a `00 00 00 01` start code in place of the length. Decoding
//!    starts at the first `stss` sync sample (absent `stss` = every
//!    sample sync, so sample 0); a track whose first sync is not
//!    sample 0 simply has no decodable meaning before that point.
//! 4. Decoded frames pair with their *own sample's* PTS (decode order),
//!    then the pairs are sorted by PTS into presentation order — the
//!    order `ctts` composition offsets encode. The fixed-rate sampler
//!    keeps the **first frame whose `floor(2·pts/timescale)` slot
//!    differs from the previously kept frame's** — the
//!    `frame_index = presentation_time · 2` rule of §4.2 at
//!    [`SAMPLE_FPS`] = 2 fps.
//! 5. Each kept frame's luma plane goes through the image pHash of
//!    kit.md §3 verbatim ([`frame_phash`]), producing an ordered
//!    `u64` chain.
//! 6. Tier-2 signature = the chain plus a 128-word MinHash over its
//!    consecutive 3-frame-hash shingles ([`minhash`]).
//!
//! [`match_score`] compares two chains *in temporal order*: index-wise
//! over `min(len)` with a frame match = Hamming ≤ [`FRAME_HAMMING_MAX`].
//!
//! Hostile input is a named [`Error`], never a panic; fragmented MP4
//! (`moof`/`mvex`) is refused by the demuxer with `Unsupported` before
//! this crate sees a byte of media data.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod avcc;
mod minhash;
mod phash;

use alloc::vec::Vec;

extern crate alloc;

use modhash_h264::{Decoder, Frame, Limits as H264Limits};
use modhash_mp4::{EntryKind, Sample, demux};
use modhash_primitives::{Digest, Error, Result, sha256};

pub use minhash::SHINGLE_FRAMES;
pub use phash::frame_phash;

/// Frames sampled per second of video — the `2` of
/// `frame_index = pts · 2` in spec §4.2.
pub const SAMPLE_FPS: f64 = 2.0;

/// Slot width denominator of the sampler: `slot = (pts · 2) /
/// timescale`, floor division. Kept as an integer so the index math is
/// exact — no float rounding can move a boundary frame between slots.
const SLOTS_PER_SEC: u64 = 2;

/// Per-frame match bound: two sampled frames count as matching when the
/// Hamming distance of their pHashes is at most this. Equal to the kit's
/// image pHash bound (`kit.md` §3, `hamming ≤ 10`) — the same 64-bit
/// hash, the same noise tolerance per frame.
pub const FRAME_HAMMING_MAX: u32 = 10;

/// Advisory `matched` bound on [`VideoMatch::score`]: a pair counts as
/// a match when at least this fraction of temporally aligned frames
/// agree. Chosen at the text lane's `0.8` (`kit.md` §4): the empirical
/// separation of same-content re-encodes (≥ 0.9) from unrelated clips
/// (≤ 0.2) leaves the bound slack on both sides.
pub const MATCH_SCORE_MIN: f64 = 0.8;

/// Ceilings a caller imposes on one [`decode`] call.
///
/// [`Limits`] is not optional: an mp4 is a table of contents that can
/// claim a million frames in a kilobyte, so fingerprinting without a
/// ceiling is a denial-of-service primitive. [`Default`] covers the
/// kit's hashing use — files up to 256 MiB, tracks up to ten minutes,
/// at most 8192 sampled frames — and is *not* a permissive mode:
/// exceeding a bound returns [`Error::TooLarge`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Limits {
    /// Hard ceiling on the container size, in bytes. Default 256 MiB.
    pub max_input: usize,
    /// Hard ceiling on the video track's `mdhd` duration, in seconds.
    /// Default 600.
    pub max_duration_s: f64,
    /// Hard ceiling on sampled (2 fps) frames in one fingerprint.
    /// Default 8192 — a touch above the 1200 a ten-minute clip samples.
    pub max_frames: usize,
    /// Hard ceiling on *decoded* frames, independent of sampling: the
    /// decoder must run every sample even when only one in fifteen is
    /// kept, so this is the real workload bound. Default 65536.
    pub max_decoded_frames: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_input: 256 * 1024 * 1024,
            max_duration_s: 600.0,
            max_frames: 8192,
            max_decoded_frames: 65536,
        }
    }
}

/// Everything the video lane computes from one file — the value
/// `match` semantics are defined over plus the facts `describe`
/// surfaces.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFingerprint {
    /// Perceptual hashes of the sampled frames, in presentation order.
    pub frame_hashes: Vec<u64>,
    /// 128-word MinHash over the chain's consecutive 3-frame shingles.
    pub minhash: Vec<u64>,
    /// Video track duration in seconds (`mdhd` duration / timescale;
    /// when `mdhd` records zero, `last_pts + last_sample_duration`).
    pub duration: f64,
    /// Achieved sampling rate: `frame_hashes.len() / duration`. Below
    /// [`SAMPLE_FPS`] whenever the source's own rate is lower or its
    /// last interval is unfilled — the configured rate stays 2 fps.
    pub fps_sampled: f64,
    /// Displayed width of the decoded stream (SPS cropping applied).
    pub width: u32,
    /// Displayed height.
    pub height: u32,
    /// Tier-1 canonical digest: SHA-256 over
    /// `width_le ∥ height_le ∥ decoded_frame_count_le` then, in
    /// presentation order, each frame's `sha256(y ∥ cb ∥ cr)` —
    /// a chained digest so memory stays O(one frame), not O(video).
    /// `u32`/`u64` little-endian headers.
    pub content_digest: Digest<32>,
}

/// The outcome of comparing two video fingerprints.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VideoMatch {
    /// Fraction of temporally aligned frames matching —
    /// [`match_score`]'s value, `0.0..=1.0`.
    pub score: f64,
    /// MinHash Jaccard estimate of the two shingle sets — the
    /// index-lookup half of the signature, reported alongside the
    /// score rather than used as the match itself.
    pub minhash_jaccard: f64,
    /// Advisory verdict: `score >= `[`MATCH_SCORE_MIN`].
    pub matched: bool,
}

/// The sync sample decoding starts from: the first `stss`-flagged
/// sample, or 0 when `stss` is absent (every sample sync).
fn first_sync(samples: &[Sample]) -> Result<usize> {
    samples
        .iter()
        .position(|s| s.keyframe)
        .ok_or(Error::BadValue("video track has no sync samples"))
}

/// Parses the first `avc1`/`avc3` `stsd` entry's `avcC` record;
/// everything else on the `vide` track is refused by name or ignored.
fn track_avcc(track: &modhash_mp4::Track) -> Result<avcc::AvcConfig> {
    let mut saw_visual = false;
    for entry in &track.table.description.entries {
        if let EntryKind::Visual { coding, avcc, .. } = entry {
            match coding {
                b"avc1" | b"avc3" => {
                    let record = avcc
                        .as_deref()
                        .ok_or(Error::BadValue("avc sample entry without avcC"))?;
                    return avcc::parse_avcc(record);
                }
                // Known non-AVC codings, refused by name. `Four` is a
                // byte array; the messages name the common shapes, the
                // fallback stays honest for the rest.
                b"hev1" | b"hvc1" => return Err(Error::Unsupported("video codec HEVC")),
                b"mp4v" => return Err(Error::Unsupported("video codec MPEG-4 part 2")),
                b"av01" => return Err(Error::Unsupported("video codec AV1")),
                b"vp09" => return Err(Error::Unsupported("video codec VP9")),
                _ => saw_visual = true,
            }
        }
    }
    if saw_visual {
        Err(Error::Unsupported("video codec not AVC"))
    } else {
        Err(Error::Unsupported("video track has no visual sample entry"))
    }
}

/// Re-frames one mp4 sample as Annex-B: each `nal_length_size`-byte
/// length prefix becomes a `00 00 00 01` start code. `out` is reused
/// across samples so peak allocation is the largest sample, not the
/// file. Returns `false` for a zero-length sample (skipped, not pushed).
fn sample_to_annexb(payload: &[u8], length_size: usize, out: &mut Vec<u8>) -> Result<bool> {
    out.clear();
    if payload.is_empty() {
        return Ok(false);
    }
    let mut pos = 0usize;
    while pos < payload.len() {
        let n = match length_size {
            1 => usize::from(
                *payload
                    .get(pos)
                    .ok_or(Error::truncated("avc NAL length", 1, 0))?,
            ),
            2 => {
                let b = payload.get(pos..pos + 2).ok_or(Error::truncated(
                    "avc NAL length",
                    2,
                    payload.len() - pos,
                ))?;
                usize::from(u16::from_be_bytes([b[0], b[1]]))
            }
            4 => {
                let b = payload.get(pos..pos + 4).ok_or(Error::truncated(
                    "avc NAL length",
                    4,
                    payload.len() - pos,
                ))?;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
            }
            // parse_avcc has already excluded 3; any other width is a
            // bug in this crate, not input data.
            _ => return Err(Error::BadValue("avcC NAL length size")),
        };
        pos += length_size;
        let end = pos
            .checked_add(n)
            .ok_or(Error::BadValue("avc NAL length overflow"))?;
        let nal = payload.get(pos..end).ok_or(Error::truncated(
            "avc NAL payload",
            n,
            payload.len() - pos,
        ))?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        pos = end;
    }
    Ok(true)
}

/// `floor(2·pts / timescale)` on the signed PTS — the sampling slot of
/// spec §4.2. `div_euclid` floors for negative `pts` too, so a clip
/// whose first frame sits at a small negative composition offset still
/// lands in one deterministic slot.
fn sample_slot(pts: i64, timescale: u32) -> i64 {
    // pts·2 fits i64 for every realistic timestamp (i64::MAX ticks of
    // overflow would need a timescale below 2 Hz at 10⁹-year lengths).
    pts.saturating_mul(SLOTS_PER_SEC as i64)
        .div_euclid(i64::from(timescale))
}

/// One decoded frame reduced to what the fingerprint keeps: its sample
/// PTS, its pHash and its per-frame content digest (so the full plane
/// can be dropped immediately).
struct Row {
    pts: i64,
    phash: u64,
    digest: Digest<32>,
}

/// Fingerprints `input` (a whole ISO-BMFF file) under `limits`.
///
/// Errors are named: [`Error::InvalidMagic`]/[`Error::Truncated`]/
/// [`Error::BadValue`] for malformed containers or streams,
/// [`Error::Unsupported`] for fragmented MP4 and non-AVC or
/// above-baseline codecs, [`Error::TooLarge`] when a [`Limits`] bound
/// is exceeded. Nothing panics.
pub fn decode(input: &[u8], limits: &Limits) -> Result<VideoFingerprint> {
    if input.len() > limits.max_input {
        return Err(Error::too_large("mp4 input", limits.max_input));
    }
    let mp4 = demux(input)?;
    let track = mp4
        .video_track()
        .ok_or(Error::BadValue("mp4 has no video track"))?;
    if track.timescale == 0 {
        return Err(Error::BadValue("video track timescale is zero"));
    }
    let cfg = track_avcc(track)?;

    // Track duration decides the `max_duration_s` gate before any byte
    // of media is touched. A zero `mdhd` duration falls back to the
    // last sample's end time so the gate still reads a real number.
    let mut samples: Vec<Sample> = Vec::new();
    for s in track.samples() {
        samples.push(s?);
    }
    if samples.is_empty() {
        return Err(Error::BadValue("video track has no samples"));
    }
    let duration_s = if track.duration != 0 {
        track.duration as f64 / f64::from(track.timescale)
    } else {
        let last = samples[samples.len() - 1];
        (last.presentation + i64::from(last.duration)) as f64 / f64::from(track.timescale)
    };
    if duration_s > limits.max_duration_s {
        return Err(Error::too_large(
            "video duration (seconds)",
            limits.max_duration_s as usize,
        ));
    }

    let start = first_sync(&samples)?;

    // Annex-B lead-in: the parameter sets avcC carries, in record
    // order. An avc3 track may legitimately carry none — in-band
    // parameter sets then arrive with the samples themselves.
    let mut lead = Vec::new();
    for nal in cfg.sps.iter().chain(cfg.pps.iter()) {
        lead.extend_from_slice(&[0, 0, 0, 1]);
        lead.extend_from_slice(nal);
    }

    let mut dec = Decoder::new(H264Limits {
        // The container bound already gated the file; per-sample pushes
        // would multiply-check a partial buffer against it.
        max_input: usize::MAX,
        max_luma_samples: H264Limits::default().max_luma_samples,
        // The cumulative cap is this crate's `max_decoded_frames`;
        // push_stream's Vec is drained every call.
        max_frames: usize::MAX,
        max_refs: H264Limits::default().max_refs,
    });
    if !lead.is_empty() {
        // Returns an empty Vec (no pictures yet); a bad SPS/PPS is a
        // named decode error here.
        dec.push_stream(&lead)?;
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut annexb: Vec<u8> = Vec::new();
    let mut dims: Option<(u32, u32)> = None;
    for (i, s) in samples.iter().enumerate().skip(start) {
        if !sample_to_annexb(
            track.sample_bytes(input, i)?,
            cfg.nal_length_size,
            &mut annexb,
        )? {
            continue;
        }
        for f in dec.push_stream(&annexb)? {
            note_frame(&mut rows, &mut dims, s.presentation, &f, limits)?;
        }
    }

    fingerprint(rows, dims, duration_s, track.timescale, limits)
}
fn note_frame(
    rows: &mut Vec<Row>,
    dims: &mut Option<(u32, u32)>,
    pts: i64,
    f: &Frame,
    limits: &Limits,
) -> Result<()> {
    if rows.len() >= limits.max_decoded_frames {
        return Err(Error::too_large(
            "decoded video frames",
            limits.max_decoded_frames,
        ));
    }
    match *dims {
        None => *dims = Some((f.width, f.height)),
        Some((w, h)) if (w, h) != (f.width, f.height) => {
            return Err(Error::BadValue("mid-stream resolution change"));
        }
        _ => {}
    }
    let phash = frame_phash(f.width, f.height, &f.y)?;
    let mut plane = Vec::with_capacity(f.y.len() + f.cb.len() + f.cr.len());
    plane.extend_from_slice(&f.y);
    plane.extend_from_slice(&f.cb);
    plane.extend_from_slice(&f.cr);
    rows.push(Row {
        pts,
        phash,
        digest: sha256(&plane)?,
    });
    Ok(())
}

/// Sorts decode-order rows into presentation order and applies the
/// 2 fps slot filter, then folds the kept frames into the fingerprint.
fn fingerprint(
    mut rows: Vec<Row>,
    dims: Option<(u32, u32)>,
    duration_s: f64,
    timescale: u32,
    limits: &Limits,
) -> Result<VideoFingerprint> {
    if rows.is_empty() {
        return Err(Error::BadValue("video track decoded to no frames"));
    }
    let (width, height) = dims.unwrap_or((0, 0));
    // Presentation order: sort by the sample's own PTS. Stable, so
    // same-PTS rows keep decode order.
    rows.sort_by_key(|r| r.pts);

    // Tier-1 content digest: chained per-frame digests in presentation
    // order — `sha256(w ∥ h ∥ n ∥ d₀ ∥ d₁ ∥ …)` where `dᵢ` is the
    // frame's own `sha256(y ∥ cb ∥ cr)`.
    let mut buf = Vec::with_capacity(16 + rows.len() * 32);
    buf.extend_from_slice(&width.to_le_bytes());
    buf.extend_from_slice(&height.to_le_bytes());
    buf.extend_from_slice(&(rows.len() as u64).to_le_bytes());
    for r in &rows {
        buf.extend_from_slice(r.digest.as_bytes());
    }
    let content_digest = sha256(&buf)?;

    // The 2 fps slot filter: keep the first frame of every new
    // `floor(2·pts/timescale)` slot. In presentation order this is the
    // earliest frame shown inside each half-second window.
    let mut frame_hashes: Vec<u64> = Vec::new();
    let mut last_slot: Option<i64> = None;
    for r in &rows {
        let slot = sample_slot(r.pts, timescale);
        if last_slot == Some(slot) {
            continue;
        }
        if frame_hashes.len() >= limits.max_frames {
            return Err(Error::too_large("sampled video frames", limits.max_frames));
        }
        frame_hashes.push(r.phash);
        last_slot = Some(slot);
    }

    let fps_sampled = if duration_s > 0.0 {
        frame_hashes.len() as f64 / duration_s
    } else {
        0.0
    };
    Ok(VideoFingerprint {
        minhash: minhash::minhash(&frame_hashes),
        frame_hashes,
        duration: duration_s,
        fps_sampled,
        width,
        height,
        content_digest,
    })
}

/// Hamming distance between two 64-bit frame hashes — the same count
/// `modhash_primitives::hamming` reports on the same words.
fn hamming64(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Fraction of frames matching in temporal order: index `i` of `a`
/// pairs with index `i` of `b` (both chains are already sorted by
/// PTS — temporal order, never file order), a pair matches when
/// `hamming ≤ `[`FRAME_HAMMING_MAX`], and the score is matches over
/// `min(a.len(), b.len())`. Two empty chains score `1.0` — the
/// `jaccard_estimate` convention for empty-vs-empty; a fingerprint
/// produced by [`decode`] is never empty.
#[must_use]
pub fn match_score(a: &[u64], b: &[u64]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 1.0;
    }
    let eq = a
        .iter()
        .zip(b.iter())
        .take(n)
        .filter(|(x, y)| hamming64(**x, **y) <= FRAME_HAMMING_MAX)
        .count();
    eq as f64 / n as f64
}

/// The spec §4.2 `match`: both the temporal-order frame-match score and
/// the MinHash Jaccard of the two fingerprints.
#[must_use]
pub fn video_match(a: &VideoFingerprint, b: &VideoFingerprint) -> VideoMatch {
    let score = match_score(&a.frame_hashes, &b.frame_hashes);
    VideoMatch {
        score,
        minhash_jaccard: modhash_text::jaccard_estimate(&a.minhash, &b.minhash),
        matched: score >= MATCH_SCORE_MIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4.2 index rule `frame_index = floor(2·pts/timescale)`,
    /// pinned at the boundary: a frame one tick either side of a
    /// half-second edge lands in different slots, and a negative
    /// composition offset floors (not truncates) downward.
    #[test]
    fn sample_slot_is_floor_of_pts_times_two() {
        // timescale 8 (8 fps): slots 0,1 = pts 0..4, 4..8.
        assert_eq!(sample_slot(0, 8), 0);
        assert_eq!(sample_slot(3, 8), 0);
        assert_eq!(sample_slot(4, 8), 1);
        assert_eq!(sample_slot(7, 8), 1);
        assert_eq!(sample_slot(8, 8), 2);
        // Odd ticks, odd timescale: pts 5 @ts=9 → floor(10/9) = 1.
        assert_eq!(sample_slot(5, 9), 1);
        assert_eq!(sample_slot(4, 9), 0);
        // Negative PTS floors: pts -1 @ts=8 → floor(-2/8) = -1, not 0.
        assert_eq!(sample_slot(-1, 8), -1);
        assert_eq!(sample_slot(-4, 8), -1);
        assert_eq!(sample_slot(-5, 8), -2);
    }

    /// Mutation witness 1 — sampling index math. If `sample_slot`
    /// truncated instead of flooring (or the x2 were dropped), the
    /// pinned assertions above and this boundary table both fail; this
    /// test additionally proves the slot filter *uses* the math by
    /// driving `fingerprint` directly.
    #[test]
    fn slot_filter_keeps_first_frame_per_slot() {
        // Rows at pts 0,1,4,5,8 on timescale 8 → slots 0,0,1,1,2.
        // Distinct phash values mark which rows were kept.
        let dig = Digest::from_bytes([0u8; 32]);
        let row = |pts, p| Row {
            pts,
            phash: p,
            digest: dig,
        };
        let rows = vec![row(0, 10), row(1, 11), row(4, 20), row(5, 21), row(8, 30)];
        let fp = fingerprint(rows, Some((16, 16)), 1.0, 8, &Limits::default()).unwrap();
        assert_eq!(fp.frame_hashes, vec![10, 20, 30]);
        // Rows arriving out of presentation order are sorted first:
        // the same five rows shuffled keep the same three hashes.
        let rows = vec![row(8, 30), row(0, 10), row(5, 21), row(1, 11), row(4, 20)];
        let fp = fingerprint(rows, Some((16, 16)), 1.0, 8, &Limits::default()).unwrap();
        assert_eq!(fp.frame_hashes, vec![10, 20, 30]);
    }

    /// Mutation witness 2 — MinHash seed usage. Without `^ seed_i`
    /// every signature collapses to `min(sm64_mix(shingle))` repeated
    /// 128 times; this test proves the live signature has per-slot
    /// diversity on a two-shingle chain, which only seeded permutations
    /// produce.
    #[test]
    fn minhash_uses_permutation_seeds() {
        let sig = minhash::minhash(&[0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD]);
        assert_eq!(sig.len(), 128);
        // Two shingles, 128 permutations: the per-slot minima differ —
        // each seed picks independently between the two shingle hashes.
        // An unseeded fold (`min(sm64_mix(shingle))` in every slot)
        // would repeat ONE value 128 times.
        let distinct: std::collections::BTreeSet<u64> = sig.iter().copied().collect();
        assert!(
            distinct.len() > 2,
            "unseeded minhash collapses to 1; got {} distinct",
            distinct.len()
        );
    }

    /// Mutation witness 3 — temporal-order match. Reversing the chain
    /// must change the score because index i pairs with index i; an
    /// order-insensitive implementation (sorted or set-compare) would
    /// score this pair 1.0.
    #[test]
    fn match_score_is_order_sensitive() {
        let a = [0u64, u64::MAX, 0x0F0F_0F0F_0F0F_0F0F, 42];
        let mut b = a;
        b.reverse();
        let forward = match_score(&a, &b);
        assert!(
            forward < 1.0,
            "order-insensitive scoring would return 1.0, got {forward}"
        );
        // And identical order is still perfect.
        assert_eq!(match_score(&a, &a), 1.0);
    }
}
