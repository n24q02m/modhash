//! Conformance for `modhash-audio`: every signal below is synthesized
//! in this file, so expected values are pinned against the pipeline's
//! documented constants, not an external fingerprint tool. RIFF bytes
//! are built byte-by-byte the way `modhash-wav`'s tests do.

use modhash_audio::{
    DELTA_TOL, HOP, SAMPLE_RATE, Signature, WINDOW, build_index, match_signature, signature,
    signature_of_flac, signature_of_wav,
};

// ---- deterministic PRNG -----------------------------------------------------
// SplitMix64, identical bit stream to modhash_primitives::SplitMix64 —
// duplicated here because the DAG gives this crate no edge to
// primitives (test code may not add edges either: cargo metadata's
// dependency list includes dev-dependencies).

struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

// ---- signal synthesis -------------------------------------------------------
/// Musical-ish synthetic content: `VOICES` sinusoids parked one per
/// slot band (slots 5,10,…,50 — five slots apart so Hann side lobes
/// don't fight the strict-left frequency rule). Each voice is silent
/// except during PRNG-scheduled bursts that attack at amplitude A and
/// decay ×0.82 per frame for ~10 frames — every burst's first frame is
/// a strict temporal local maximum, so the signature is rich and
/// reproducible.
fn synth(seed: u64, n_samples: usize) -> Vec<i32> {
    const VOICES: usize = 10;
    let n_frames = n_samples / HOP;
    let mut rng = SplitMix64::new(seed);
    // Voice carrier frequencies pinned near slot centres.
    let freqs: [f64; VOICES] = {
        let mut f = [0.0; VOICES];
        let mut i = 0;
        while i < VOICES {
            let slot = 5 + 5 * i; // slots 5..50
            let bin = (slot * 8 + 3) as f64; // slot centre bin
            f[i] = bin * SAMPLE_RATE as f64 / WINDOW as f64;
            i += 1;
        }
        f
    };
    // Per-voice, per-frame amplitude envelope: bursts of attack+decay.
    let mut env = vec![vec![0.0f64; n_frames]; VOICES];
    for v in env.iter_mut() {
        for fr in 0..n_frames {
            let r = rng.next_u64();
            if r % 23 == 0 {
                // Burst: attack A then ×0.82 decay over ~10 frames.
                let a = 0.5 + 0.45 * ((r >> 32) as f64 / u64::MAX as f64);
                let len = 10;
                for k in 0..len.min(n_frames - fr) {
                    v[fr + k] = a * 0.82f64.powi(k as i32);
                }
            }
        }
    }
    let mut out = vec![0i32; n_samples];
    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f64 / SAMPLE_RATE as f64;
        let mut x = 0.0;
        for (v, f) in freqs.iter().enumerate() {
            let a = env[v][(i / HOP).min(n_frames - 1)];
            if a > 0.0 {
                x += a * (2.0 * core::f64::consts::PI * f * t).sin();
            }
        }
        *s = (x.clamp(-0.95, 0.95) * 2147483647.0) as i32;
    }
    out
}

// ---- RIFF fixture builder (same recipe as modhash-wav tests) ----------------

fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len() + 1);
    v.extend_from_slice(id);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// Minimal PCM16 WAV at the requested rate/channels from i32 samples
/// (high 16 bits are written — matches the decoder's scale path).
fn wav16(samples: &[i32], channels: u16, rate: u32) -> Vec<u8> {
    let block_align = channels * 2;
    let mut fmt = Vec::with_capacity(16);
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let mut data = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        data.extend_from_slice(&((*s >> 16) as i16).to_le_bytes());
    }
    let body_len = 4 + 8 + fmt.len() + 8 + data.len();
    let mut v = Vec::with_capacity(8 + body_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(body_len as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(&chunk(b"fmt ", &fmt));
    v.extend_from_slice(&chunk(b"data", &data));
    v
}

// ---- helpers ----------------------------------------------------------------

const SEC: usize = SAMPLE_RATE as usize; // mono samples per second
const OFFSET_2S: usize = 2 * SEC; // the spec's pinned offset: 88 200

fn sig_of(pcm: &[i32]) -> Signature {
    signature(pcm, 1).expect("mono i32 stream must be accepted")
}

/// Top match's id, asserting a non-empty result.
fn top_id(query: &Signature, index: &modhash_audio::Index) -> u32 {
    let m = match_signature(query, index);
    assert!(!m.is_empty(), "expected at least one match");
    m[0].id
}

// ---- acceptance: determinism ------------------------------------------------

#[test]
fn same_wav_twice_gives_identical_signature() {
    let pcm = synth(0xA0, 4 * SEC);
    let bytes = wav16(&pcm, 1, SAMPLE_RATE);
    let a = signature_of_wav(&bytes).expect("valid wav");
    let b = signature_of_wav(&bytes).expect("valid wav");
    assert_eq!(a, b);
    assert!(!a.is_empty(), "synthetic content must produce peaks");
    // The facade must agree with the raw-PCM path on the same stream.
    let direct = sig_of(modhash_wav::decode(&bytes).unwrap().samples());
    let via_facade = sig_of(&pcm.iter().map(|s| s >> 16 << 16).collect::<Vec<_>>());
    assert_eq!(
        direct, via_facade,
        "16-bit truncation is the only scale step"
    );
}

#[test]
fn signature_is_reproducible_from_pcm() {
    let pcm = synth(0xB1, 3 * SEC);
    assert_eq!(sig_of(&pcm), sig_of(&pcm));
}

// ---- acceptance: 2 s offset -------------------------------------------------

#[test]
fn two_second_offset_recovers_id_and_delta() {
    let content = synth(0xC2, 4 * SEC);
    let prefix = synth(0xD3, OFFSET_2S);
    let mut shifted = prefix;
    shifted.extend_from_slice(&content);

    let index = build_index(&[sig_of(&content), sig_of(&synth(0xE4, 4 * SEC))]);
    let m = match_signature(&sig_of(&shifted), &index);
    assert!(!m.is_empty(), "offset query must match");
    assert_eq!(m[0].id, 0, "content is corpus member 0");
    // 88 200 samples / 2048-hop = 43.066 frames; the ±1-frame tolerance
    // covers the rounding.
    let expected = OFFSET_2S as f64 / HOP as f64;
    assert!(
        (m[0].delta_t as f64 - expected).abs() <= f64::from(DELTA_TOL),
        "delta_t {} not within ±{} frame of {:.3}",
        m[0].delta_t,
        DELTA_TOL,
        expected
    );
    // The modal bin must carry a decisive share of the content's own
    // peaks — not merely outvote incidental noise.
    let content_peaks = sig_of(&content).len();
    assert!(
        m[0].votes as usize * 2 >= content_peaks,
        "votes {} should cover ≥half of {content_peaks} content peaks",
        m[0].votes,
    );
}

// ---- acceptance: Δt histogram closeness across offsets ----------------------

#[test]
fn delta_t_histogram_tracks_arbitrary_offsets() {
    let content = synth(0xF5, 5 * SEC);
    let index = build_index(&[sig_of(&content)]);
    for &offset in &[SEC / 2, SEC, OFFSET_2S, 37 * SEC / 10] {
        let prefix = synth(offset as u64 ^ 0x77, offset);
        let mut q = prefix;
        q.extend_from_slice(&content);
        let m = match_signature(&sig_of(&q), &index);
        assert_eq!(top_id(&sig_of(&q), &index), 0);
        let expected = offset as f64 / HOP as f64;
        assert!(
            (m[0].delta_t as f64 - expected).abs() <= f64::from(DELTA_TOL),
            "offset {offset} samples: Δt {} vs {:.3}",
            m[0].delta_t,
            expected
        );
        assert!(
            m[0].votes >= 8,
            "offset {offset}: only {} votes",
            m[0].votes
        );
    }
}

#[test]
fn empty_query_and_foreign_content_behave() {
    let content = synth(0x77, 3 * SEC);
    let content_sig = sig_of(&content);
    let index = build_index(std::slice::from_ref(&content_sig));
    assert!(match_signature(&sig_of(&[]), &index).is_empty());
    // Self-match is the reference vote mass for this content.
    let real_votes = match_signature(&content_sig, &index)[0].votes;
    // A different synth may match zero ids, or match with strictly
    // fewer votes than the real content.
    let foreign = sig_of(&synth(0x88, 3 * SEC));
    if let Some(top) = match_signature(&foreign, &index).first() {
        assert!(top.votes < real_votes);
    }
}

// ---- behaviour pins ----------------------------------------------------------

#[test]
fn stereo_mixdown_matches_explicit_average() {
    let left = synth(0x11, 2 * SEC);
    let right = synth(0x22, 2 * SEC);
    let mut stereo = Vec::with_capacity(left.len() * 2);
    for i in 0..left.len() {
        stereo.push(left[i]);
        stereo.push(right[i]);
    }
    let mono: Vec<i32> = (0..left.len())
        .map(|i| ((i64::from(left[i]) + i64::from(right[i])) / 2) as i32)
        .collect();
    assert_eq!(signature(&stereo, 2).unwrap(), signature(&mono, 1).unwrap());
}

#[test]
fn fingerprint_is_deterministic_and_signal_dependent() {
    let a = sig_of(&synth(0x33, 3 * SEC));
    let b = sig_of(&synth(0x44, 3 * SEC));
    assert_eq!(a.fingerprint(), sig_of(&synth(0x33, 3 * SEC)).fingerprint());
    assert_ne!(a.fingerprint(), b.fingerprint());
    assert_ne!(a.fingerprint(), 0);
}

// ---- hostile input ------------------------------------------------------------

#[test]
fn wav_facade_rejects_wrong_rate_and_garbage() {
    let pcm = synth(0x99, SEC / 2);
    let lo = wav16(&pcm, 1, 22_050);
    match signature_of_wav(&lo) {
        Err(modhash_audio::Error::Unsupported(_)) => {}
        other => panic!("22 050 Hz wav must be Unsupported, got {other:?}"),
    }
    match signature_of_wav(b"not a wav") {
        Err(modhash_audio::Error::Decode(_)) => {}
        other => panic!("garbage must be Decode, got {other:?}"),
    }
    match signature_of_flac(b"not a flac") {
        Err(modhash_audio::Error::Decode(_)) => {}
        other => panic!("garbage must be Decode, got {other:?}"),
    }
}

#[test]
fn zero_channels_and_partial_frame_err_not_panic() {
    assert!(signature(&[0; 64], 0).is_err());
    assert!(signature(&[0; 65], 2).is_err());
    // Extreme values must not overflow the i64 mixdown or panic.
    let loud = vec![i32::MAX; WINDOW * 2];
    assert!(signature(&loud, 4).is_ok());
}
