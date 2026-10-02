//! The kit-level error type: every fallible facade call returns it.
//!
//! Two shapes cover the kit: codec rejections arrive as
//! [`modhash_primitives::Error`] and are wrapped with the modality name
//! in [`Error::Decode`]; `modhash_audio` — `no_std`, no edge to
//! primitives — carries its own [`modhash_audio::Error`], wrapped as
//! [`Error::Audio`]. `Display` for both spells the modality name first,
//! so an error read from a log always says *which* pipeline refused.

use crate::Modality;
use core::fmt;
use modhash_primitives::Format;

/// Every failure the facade can report.
///
/// `Copy` is impossible: [`Error::Audio`] holds a `String` through
/// `modhash_audio::Error`. The type still avoids allocating on the hot
/// paths — codec rejections move a `Copy` inner error.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A modality decoder or the chunker rejected the input. `source`
    /// is the codec's own error; `modality` names the pipeline that
    /// refused, because the same `Error::Truncated` means a different
    /// file in "image:" than in "audio:".
    Decode {
        /// The pipeline that rejected the bytes.
        modality: Modality,
        /// The codec's own typed error.
        source: modhash_primitives::Error,
    },
    /// A recognized format whose modality lane exists in the design
    /// (mp3 → audio, mp4 → video, pdf → text) but whose crate has not
    /// landed yet. This is a refusal to guess, never a failure to parse
    /// — the bytes may be perfectly valid mp4 and the kit still says
    /// "not yet" instead of hashing container noise.
    Unsupported {
        /// The modality the unlanded crate will serve.
        modality: Modality,
        /// The detected container format.
        format: Format,
        /// What is missing, e.g. `"modhash-mp3 decoder has not landed"`.
        what: &'static str,
    },
    /// The PDF text layer's own error type (carries page/object
    /// attribution and cannot fold into [`modhash_primitives::Error`]).
    Pdf(modhash_pdf::Error),
    /// The audio facade's own error type (`modhash_audio::Error`
    /// carries a `String` and cannot fold into
    /// [`modhash_primitives::Error`]).
    Audio(modhash_audio::Error),
    /// A structurally valid but semantically impossible call:
    /// `match` on two signatures of different modalities, or text
    /// input that is not valid UTF-8.
    BadValue(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Decode { modality, source } => {
                f.write_str(modality.as_str())?;
                f.write_str(": ")?;
                fmt::Display::fmt(source, f)
            }
            Error::Unsupported {
                modality,
                format,
                what,
            } => {
                f.write_str(modality.as_str())?;
                f.write_str(" (")?;
                f.write_str(format.as_str())?;
                f.write_str("): ")?;
                f.write_str(what)
            }
            Error::Audio(e) => {
                f.write_str("audio: ")?;
                fmt::Display::fmt(e, f)
            }
            Error::Pdf(e) => {
                f.write_str("text: ")?;
                fmt::Display::fmt(e, f)
            }
            Error::BadValue(what) => {
                f.write_str("bad value: ")?;
                f.write_str(what)
            }
        }
    }
}

impl core::error::Error for Error {}

impl From<modhash_pdf::Error> for Error {
    fn from(e: modhash_pdf::Error) -> Self {
        Error::Pdf(e)
    }
}

impl From<modhash_audio::Error> for Error {
    fn from(e: modhash_audio::Error) -> Self {
        Error::Audio(e)
    }
}
