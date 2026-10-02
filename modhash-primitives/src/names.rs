//! The closed sets of names that appear in on-disk records.

use core::fmt;

/// The hash functions this workspace can produce, so an index record
/// can say what it was computed with.
///
/// The [`as_str`](Algorithm::as_str) / [`from_str`](Algorithm::from_str)
/// pair is the stable wire form: these strings appear in on-disk
/// records and must not change without a format version bump.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Algorithm {
    /// SHA-1. 20-byte digests.
    Sha1,
    /// SHA-2 with a 256-bit state. 32-byte digests.
    Sha256,
    /// SHA-2 with a 512-bit state. 64-byte digests.
    Sha512,
    /// BLAKE3 with the default 32-byte output.
    Blake3,
    /// MD5. 16-byte digests.
    Md5,
    /// xxHash64. 8-byte digests.
    XxHash64,
    /// FastCDC content-defined chunking. 4-byte digests: the 32-bit
    /// Gear fingerprint at the cut point, the width the FastCDC
    /// reference implementation produces.
    FastCdc,
    /// A perceptual hash. The width is a property of the modality
    /// (image, audio, video, text), so
    /// [`output_len`](Algorithm::output_len) is `None`.
    Perceptual,
    /// A name written by a future version of this kit. A record
    /// carrying it can still be read; it just cannot be recomputed here.
    Unknown,
}

impl Algorithm {
    /// The stable wire form, e.g. `"sha256"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Algorithm::Sha1 => "sha1",
            Algorithm::Sha256 => "sha256",
            Algorithm::Sha512 => "sha512",
            Algorithm::Blake3 => "blake3",
            Algorithm::Md5 => "md5",
            Algorithm::XxHash64 => "xxhash64",
            Algorithm::FastCdc => "fastcdc",
            Algorithm::Perceptual => "perceptual",
            Algorithm::Unknown => "unknown",
        }
    }

    /// Parses the wire form. An unrecognised string yields
    /// [`Algorithm::Unknown`], never an error, so records written by a
    /// future version stay readable. Matching is exact and
    /// case-sensitive: `"SHA256"` is `Unknown`.
    pub fn from_str(s: &str) -> Self {
        match s {
            "sha1" => Algorithm::Sha1,
            "sha256" => Algorithm::Sha256,
            "sha512" => Algorithm::Sha512,
            "blake3" => Algorithm::Blake3,
            "md5" => Algorithm::Md5,
            "xxhash64" => Algorithm::XxHash64,
            "fastcdc" => Algorithm::FastCdc,
            "perceptual" => Algorithm::Perceptual,
            _ => Algorithm::Unknown,
        }
    }

    /// Digest width in bytes for the fixed-width members; `None` for
    /// [`Perceptual`](Algorithm::Perceptual), whose width is a property
    /// of the modality, and for [`Unknown`](Algorithm::Unknown).
    pub fn output_len(&self) -> Option<usize> {
        match self {
            Algorithm::Sha1 => Some(20),
            Algorithm::Sha256 => Some(32),
            Algorithm::Sha512 => Some(64),
            Algorithm::Blake3 => Some(32),
            Algorithm::Md5 => Some(16),
            Algorithm::XxHash64 => Some(8),
            Algorithm::FastCdc => Some(4),
            Algorithm::Perceptual | Algorithm::Unknown => None,
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The container formats the codecs decode.
///
/// The enum is the set of formats the workspace *claims* to handle. A
/// member is added in the phase that implements that decoder, not
/// before — an enum listing a format that does not decode is a lie a
/// caller can act on. The [`as_str`](Format::as_str) /
/// [`from_str`](Format::from_str) pair is the stable wire form, with
/// the same rules as [`Algorithm`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Format {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// BMP.
    Bmp,
    /// ZIP.
    Zip,
    /// MP4.
    Mp4,
    /// MP3.
    Mp3,
    /// WAV.
    Wav,
    /// FLAC.
    Flac,
    /// PDF.
    Pdf,
    /// GIF.
    Gif,
    /// A format written by a future version of this kit.
    Unknown,
}

impl Format {
    /// The stable wire form, e.g. `"png"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpeg",
            Format::Bmp => "bmp",
            Format::Zip => "zip",
            Format::Mp4 => "mp4",
            Format::Mp3 => "mp3",
            Format::Wav => "wav",
            Format::Flac => "flac",
            Format::Pdf => "pdf",
            Format::Gif => "gif",
            Format::Unknown => "unknown",
        }
    }

    /// Parses the wire form. An unrecognised string yields
    /// [`Format::Unknown`], never an error, so records written by a
    /// future version stay readable. Matching is exact and
    /// case-sensitive.
    pub fn from_str(s: &str) -> Self {
        match s {
            "png" => Format::Png,
            "jpeg" => Format::Jpeg,
            "bmp" => Format::Bmp,
            "zip" => Format::Zip,
            "mp4" => Format::Mp4,
            "mp3" => Format::Mp3,
            "wav" => Format::Wav,
            "flac" => Format::Flac,
            "pdf" => Format::Pdf,
            "gif" => Format::Gif,
            _ => Format::Unknown,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
