//! Audio fingerprint facade: spectral peaks and the peak-to-id
//! reverse map.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in
//! this workspace builds without a single registry package, enforced
//! by `scripts/gate_zero_dep.sh`, and the workspace ordering
//! (`scripts/gate_dag.py`) makes this crate #13 — above
//! `modhash-math` (the FFT), `modhash-wav` and `modhash-flac` (the
//! decoders it fronts).
//!
//! The pipeline is `docs/algorithms/audio-peaks.md` — design spec
//! §4.2: mono-mixed 44 100 Hz `i32` PCM → Hann-windowed 4096 real FFT
//! at a 2048 hop → 2049 bins max-pooled into 257 slots (bin stride 8)
//! → in-band (≈32–5000 Hz, bins 3–464 → slots 0–58) local maxima that
//! must also win their 9-frame temporal neighbourhood → the signature:
//! a sorted `(t, f)` set. Matching inverts the map — `f → (t, id)` —
//! and histograms `Δt = t_query − t_stored`; the modal offset within
//! ±1 frame is the answer.
//!
//! The crate is `no_std` apart from the `alloc` containers its API
//! returns and the error strings it carries.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::{String, ToString};
use core::fmt;

mod peaks;
mod table;

pub use peaks::{
    BIN_MAX, BIN_MIN, BIN_STRIDE, HOP, Peak, SAMPLE_RATE, SLOT_COUNT, SLOT_MAX, SLOT_MIN,
    Signature, TIME_RADIUS, WINDOW, signature,
};
pub use table::{DELTA_TOL, Index, Match, build_index, match_signature};

/// Crate-local result alias; `E` defaults to this crate's [`Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Every failure this crate can report. Deliberately narrower than
/// `modhash_primitives::Error` — the DAG gives this crate no edge to
/// primitives, so decoder errors arrive as text via [`Error::Decode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A structurally valid but semantically impossible call: zero
    /// channels, a partial trailing frame.
    BadValue(&'static str),
    /// Input of a kind this pipeline is not tuned for — anything but
    /// 44 100 Hz through the decode facades.
    Unsupported(&'static str),
    /// The WAV/FLAC facade's decoder rejected the bytes. Carries the
    /// decoder's `Display` text; the underlying typed error stays in
    /// the decoder crate.
    Decode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadValue(what) => {
                f.write_str("bad value: ")?;
                f.write_str(what)
            }
            Error::Unsupported(what) => {
                f.write_str("unsupported: ")?;
                f.write_str(what)
            }
            Error::Decode(what) => {
                f.write_str("decode: ")?;
                f.write_str(what)
            }
        }
    }
}

/// Decodes `bytes` as RIFF/WAVE via [`modhash_wav`] and extracts the
/// signature. Anything but 44 100 Hz is refused before a single FFT:
/// the constants are tuned to one rate and silently resampling is not
/// this crate's job.
///
/// # Errors
///
/// [`Error::Decode`] for a file `modhash_wav` rejects, and
/// [`Error::BadValue`] / [`Error::Unsupported`] exactly as
/// [`signature`] documents plus the sample-rate refusal.
pub fn signature_of_wav(bytes: &[u8]) -> Result<Signature> {
    let wav = modhash_wav::decode(bytes).map_err(|e| Error::Decode(e.to_string()))?;
    if wav.sample_rate() != SAMPLE_RATE {
        return Err(Error::Unsupported("wav sample rate ≠ 44100 Hz"));
    }
    signature(wav.samples(), wav.channels())
}

/// Decodes `bytes` as FLAC via [`modhash_flac`] with its default
/// [`modhash_flac::Limits`] and extracts the signature. Same 44 100 Hz
/// contract as [`signature_of_wav`].
///
/// # Errors
///
/// [`Error::Decode`] for a stream `modhash_flac` rejects, and
/// [`Error::BadValue`] / [`Error::Unsupported`] as [`signature`]
/// documents plus the sample-rate refusal.
pub fn signature_of_flac(bytes: &[u8]) -> Result<Signature> {
    let flac = modhash_flac::decode(bytes, &modhash_flac::Limits::default())
        .map_err(|e| Error::Decode(e.to_string()))?;
    if flac.sample_rate() != SAMPLE_RATE {
        return Err(Error::Unsupported("flac sample rate ≠ 44100 Hz"));
    }
    signature(flac.samples(), flac.channels())
}
