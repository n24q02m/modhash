//! Spectral-peak extraction: windowed real FFT, slot quantization and
//! Shazam-style landmark picking. Every constant lives in
//! `docs/algorithms/audio-peaks.md`; this file is the pinned
//! transcription of it.

use alloc::vec::Vec;

use modhash_math::fft_real;

use crate::{Error, Result};

/// Fixed input rate. Every constant in this pipeline is tuned to
/// 44 100 Hz; the facades refuse anything else.
pub const SAMPLE_RATE: u32 = 44_100;

/// Analysis window length in samples (≈ 92.9 ms), Hann-shaped.
pub const WINDOW: usize = 4096;

/// Frame advance in samples (≈ 46.4 ms); `WINDOW / 2`, i.e. 50 %
/// overlap.
pub const HOP: usize = 2048;

/// Number of real-FFT bins folded into one quantization slot.
pub const BIN_STRIDE: usize = 8;

/// Quantized frequency slots: 2049 real-FFT bins → `0..=256`.
pub const SLOT_COUNT: usize = 257;

/// First in-band FFT bin (≈ 32 Hz).
pub const BIN_MIN: usize = 3;

/// Last in-band FFT bin (≈ 5000 Hz), inclusive.
pub const BIN_MAX: usize = 464;

/// First slot reachable by the in-band bins (`BIN_MIN / BIN_STRIDE`).
pub const SLOT_MIN: usize = 0;

/// Last slot reachable by the in-band bins (`BIN_MAX / BIN_STRIDE`).
pub const SLOT_MAX: usize = 58;

/// Half-width of the temporal peak window: 2·4+1 = 9 frames (≈ 418 ms).
pub const TIME_RADIUS: usize = 4;

/// `f` in a [`Signature`] entry is a slot index, always ≤ `SLOT_MAX`.
const _: () = assert!(SLOT_MAX <= u16::MAX as usize);

/// One spectral landmark: `t` is the frame index (each frame = `HOP`
/// samples), `f` the quantization slot `0..=SLOT_MAX`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Peak {
    /// Analysis-frame index: sample position `t * HOP`.
    pub t: u32,
    /// Quantized frequency slot `0..=SLOT_MAX` — documented in
    /// `docs/algorithms/audio-peaks.md`.
    pub f: u16,
}

/// A track's landmark set: peaks in `(t, f)` order plus the frame
/// count the extraction observed.
#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    peaks: Vec<Peak>,
    frames: u32,
}

impl Signature {
    /// The `(t, f)` landmarks, ordered by `t` then `f`.
    #[must_use]
    pub fn peaks(&self) -> &[Peak] {
        &self.peaks
    }

    /// Number of analysis frames the source covered: a signature is
    /// empty when the input was shorter than [`WINDOW`].
    #[must_use]
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Number of landmarks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peaks.len()
    }

    /// `true` when no landmark was picked — silence, stationary tones
    /// and too-short inputs all land here.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }

    /// The §4.2 fingerprint mask: for every frame `t`, its first
    /// (lowest-`f`) peak sets bit `f mod 64`; bits accumulate across
    /// frames. A presence mask for pre-screening, not the match key.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        // peaks are (t,f)-sorted, so the first peak seen per t is the
        // lowest-f one; `last_t` remembers which frame already spent
        // its bit.
        let mut bits = 0u64;
        let mut last_t = u32::MAX;
        for peak in &self.peaks {
            if peak.t != last_t {
                last_t = peak.t;
                bits |= 1u64 << (u64::from(peak.f) & 63);
            }
        }
        bits
    }
}

/// Hann coefficient `0.5·(1 − cos(2πi/(N−1)))` — periodic extension is
/// deliberate: the centre sample `i = N/2` keeps full weight and the
/// edges taper to ~`1/(N−1)`², not exactly zero.
///
/// `cos` comes from [`modhash_math::Complex::cis`].re so the crate
/// stays `no_std`-compatible (core has no `f64::cos`).
fn hann(i: usize) -> f64 {
    let theta = -2.0 * core::f64::consts::PI * (i as f64) / ((WINDOW - 1) as f64);
    0.5 * (1.0 - modhash_math::Complex::cis(theta).re)
}

/// Extracts the landmark signature from interleaved `i32` PCM at
/// [`SAMPLE_RATE`], mixed down to mono across `channels`.
///
/// Mixing accumulates in `i64` and divides by the channel count —
/// truncation toward zero is deterministic and overflow-free for any
/// channel count an `i32` sample stream can carry.
///
/// # Errors
///
/// * [`Error::BadValue`] on `channels == 0` (would divide by zero) or
///   when `samples.len()` is not a whole number of interleaved frames —
///   a partial trailing frame is a caller bug, never guessed at.
///
/// Fewer than [`WINDOW`] samples is not an error: it yields an empty
/// signature.
pub fn signature(samples: &[i32], channels: u16) -> Result<Signature> {
    if channels == 0 {
        return Err(Error::BadValue("channels == 0"));
    }
    let ch = usize::from(channels);
    if samples.len() % ch != 0 {
        return Err(Error::BadValue(
            "samples not a whole number of interleaved frames",
        ));
    }
    let mono_frames = samples.len() / ch;
    let n_frames = if mono_frames >= WINDOW {
        (mono_frames - WINDOW) / HOP + 1
    } else {
        0
    };
    let mut peaks = Vec::new();
    if n_frames == 0 {
        return Ok(Signature { peaks, frames: 0 });
    }

    // Slot magnitudes per frame: row-major [frame][SLOT_COUNT].
    let mut mags = alloc::vec![0.0f64; n_frames * SLOT_COUNT];
    let mut frame_buf = alloc::vec![0.0f64; WINDOW];
    for fr in 0..n_frames {
        let base = fr * HOP;
        for (i, x) in frame_buf.iter_mut().enumerate() {
            let mut acc: i64 = 0;
            for c in 0..ch {
                acc += i64::from(samples[(base + i) * ch + c]);
            }
            *x = (acc / (ch as i64)) as f64 * hann(i) * (1.0 / 2147483648.0);
        }
        let spec = fft_real(&frame_buf);
        let row = &mut mags[fr * SLOT_COUNT..(fr + 1) * SLOT_COUNT];
        for (b, c) in spec.iter().enumerate().take(BIN_MAX + 1).skip(BIN_MIN) {
            let m = c.norm_sq();
            let slot = &mut row[b / BIN_STRIDE];
            if m > *slot {
                *slot = m;
            }
        }
    }

    for s in SLOT_MIN..=SLOT_MAX {
        for fr in 0..n_frames {
            let m = mags[fr * SLOT_COUNT + s];
            if m <= 0.0 {
                continue;
            }
            // Frequency local maximum inside the band: strict-left,
            // non-strict-right, so a flat top lands on the lowest slot.
            let left = if s > SLOT_MIN {
                mags[fr * SLOT_COUNT + s - 1]
            } else {
                -1.0
            };
            let right = if s < SLOT_MAX {
                mags[fr * SLOT_COUNT + s + 1]
            } else {
                -1.0
            };
            if !(m > left && m >= right) {
                continue;
            }
            // Temporal maximum over the clipped 9-frame window, strict
            // on BOTH sides: a peak must beat every neighbour, so a
            // plateau yields no peak — the deterministic reading of
            // "local maximum", and why a stationary spectrum produces
            // zero landmarks.
            let lo = fr.saturating_sub(TIME_RADIUS);
            let hi = (fr + TIME_RADIUS).min(n_frames - 1);
            let mut is_peak = true;
            for g in lo..=hi {
                if g != fr && mags[g * SLOT_COUNT + s] >= m {
                    is_peak = false;
                    break;
                }
            }
            if is_peak {
                peaks.push(Peak {
                    t: fr as u32,
                    f: s as u16,
                });
            }
        }
    }
    // (s, fr) iteration order yields (f, t) order; the contract wants
    // (t, f). Peak's Ord is field order, so this is one in-place sort.
    peaks.sort_unstable();
    Ok(Signature {
        peaks,
        frames: n_frames as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructs a signature directly — the unit-test-only path that
    /// pins `fingerprint`'s first-peak-per-frame rule without needing
    /// spectrum-shaped input.
    fn sig(pairs: &[(u32, u16)]) -> Signature {
        Signature {
            peaks: pairs.iter().map(|&(t, f)| Peak { t, f }).collect(),
            frames: pairs.iter().map(|p| p.0 + 1).max().unwrap_or(0),
        }
    }

    #[test]
    fn fingerprint_sets_first_peak_bit_per_frame() {
        // t=1 lowest f is 3 (bit 3); t=2 lowest f is 70 → 70 mod 64 = 6.
        let s = sig(&[(1, 3), (1, 40), (2, 70), (2, 130)]);
        assert_eq!(s.fingerprint(), (1 << 3) | (1 << 6));
    }

    #[test]
    fn fingerprint_of_empty_is_zero() {
        assert_eq!(sig(&[]).fingerprint(), 0);
        assert!(sig(&[]).is_empty());
    }

    #[test]
    fn hann_tapers_edges_and_is_symmetric() {
        // Symmetric Hann w(i) = 0.5(1 − cos(2πi/(N−1))): edges ~0,
        // peak between the two centre samples, mirror-symmetric.
        assert!(hann(0).abs() < 1e-9);
        assert!(hann(WINDOW - 1).abs() < 1e-9);
        assert!((hann(WINDOW / 2) - 1.0).abs() < 1e-3);
        assert_eq!(hann(17), hann(WINDOW - 1 - 17));
        assert!(hann(0) < hann(1) && hann(1) < hann(2));
    }

    #[test]
    fn rejects_zero_channels_and_partial_frames() {
        assert_eq!(
            signature(&[0; 10], 0),
            Err(Error::BadValue("channels == 0"))
        );
        assert_eq!(
            signature(&[0; 10], 3),
            Err(Error::BadValue(
                "samples not a whole number of interleaved frames"
            ))
        );
    }

    #[test]
    fn short_input_and_silence_yield_empty_signatures() {
        // Under one window: no frames at all.
        let short = signature(&[0; WINDOW - 1], 1).unwrap();
        assert_eq!(short.frames(), 0);
        assert!(short.is_empty());
        // Silence across 8 whole windows: (8·4096−4096)/2048+1 = 15
        // frames exist, but silence must never peak.
        let silent = signature(&[0; WINDOW * 8], 1).unwrap();
        assert_eq!(silent.frames(), 15);
        assert!(silent.is_empty());
    }

    #[test]
    fn stationary_tone_produces_no_temporal_peaks() {
        // A bin-aligned tone (bin 100: 2π·100·i/4096 rad/sample) is
        // phase-invariant under the 2048 hop — every frame's windowed
        // samples are bit-identical up to sign, so every (t, f) is an
        // exact tie and the earliest-wins rule rejects all of them.
        let n = WINDOW * 64;
        let mut pcm = alloc::vec::Vec::with_capacity(n);
        for i in 0..n {
            let x = (i as f64 * (2.0 * core::f64::consts::PI * 100.0 / WINDOW as f64)).sin() * 0.5;
            pcm.push((x * 2147483647.0) as i32);
        }
        let s = signature(&pcm, 1).unwrap();
        assert_eq!(s.frames(), 127);
        assert!(
            s.is_empty(),
            "stationary tone must yield zero peaks, got {}",
            s.len()
        );
    }
}
