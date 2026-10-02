//! The fixed-width digest value and the byte source it is read from.

use core::fmt;

use crate::error::{Error, Result};

/// The lowercase hex alphabet [`Digest::to_hex`] emits.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// A fixed-width hash value of `N` bytes.
///
/// `N` is the output width in bytes. The value is copied around by
/// value: it is small, and hex output in this workspace is compared
/// byte-for-byte against specification vectors constantly, so the fixed
/// array is the whole point — [`to_hex`](Digest::to_hex) returns
/// `[u8; 2 * N]`, never a heap string.
///
/// [`Default`] is the all-zero value and is *not* a valid hash of
/// anything: a zeroed buffer is what a failed hash computation tends to
/// leave behind, and it must not be mistaken for a result.
///
/// Ordering is bytewise lexicographic on the raw bytes, matching
/// `[u8; N]` itself, so digests can serve as index keys.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest<const N: usize> {
    /// The raw bytes of the digest.
    bytes: [u8; N],
}

// Manual, not derived: `Default` for `[T; N]` only exists up to
// `N <= 32`, and a 64-byte SHA-512 digest must still default to zero.
impl<const N: usize> Default for Digest<N> {
    /// The all-zero digest. Not a valid hash of anything; see the type
    /// documentation.
    fn default() -> Self {
        Digest { bytes: [0u8; N] }
    }
}

impl<const N: usize> Digest<N> {
    /// Wraps `N` bytes. Never fails.
    ///
    /// ```
    /// let d = modhash_primitives::Digest::<2>::from_bytes([0xaa, 0x55]);
    /// assert_eq!(d.as_bytes(), &[0xaa, 0x55]);
    /// ```
    pub fn from_bytes(bytes: [u8; N]) -> Self {
        Digest { bytes }
    }

    /// Borrows the raw `N` bytes.
    pub fn as_bytes(&self) -> &[u8; N] {
        &self.bytes
    }

    /// Lowercase hexadecimal as a fixed `[u8; 2 * N]` array.
    ///
    /// No allocation and no `hex` crate: the output is a fixed-size
    /// array precisely so comparing against a specification vector never
    /// pays for a heap string. Leading zeroes are kept — the output is
    /// exactly `2 * N` characters.
    ///
    /// ```
    /// let d = modhash_primitives::Digest::<2>::from_bytes([0x00, 0xff]);
    /// assert_eq!(&d.to_hex()[..], b"00ff");
    /// ```
    pub fn to_hex(self) -> [u8; 2 * N] {
        let mut out = [0u8; 2 * N];
        for (i, &byte) in self.bytes.iter().enumerate() {
            out[2 * i] = HEX[usize::from(byte >> 4)];
            out[2 * i + 1] = HEX[usize::from(byte & 0x0f)];
        }
        out
    }

    /// Parses exactly `2 * N` hexadecimal characters, lowercase or
    /// uppercase.
    ///
    /// Rejections are named: an odd-length input is
    /// [`Error::BadValue`](`crate::Error::BadValue`) `"hex length is
    /// odd"`, an even input shorter than `2 * N` is
    /// [`Error::Truncated`](`crate::Error::Truncated`), an input longer
    /// than `2 * N` is [`Error::BadValue`](`crate::Error::BadValue`)
    /// `"hex digest length"`, and any non-hex byte is
    /// [`Error::BadValue`](`crate::Error::BadValue`) `"hex digit"`.
    ///
    /// ```
    /// let d = modhash_primitives::Digest::<4>::from_hex(b"DeadBeef").unwrap();
    /// assert_eq!(d.as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
    /// assert_eq!(
    ///     modhash_primitives::Digest::<4>::from_hex(b"odd"),
    ///     Err(modhash_primitives::Error::BadValue("hex length is odd"))
    /// );
    /// ```
    pub fn from_hex(hex: &[u8]) -> Result<Self> {
        if hex.len() % 2 != 0 {
            return Err(Error::BadValue("hex length is odd"));
        }
        if hex.len() < 2 * N {
            return Err(Error::truncated("hex digest", 2 * N, hex.len()));
        }
        if hex.len() > 2 * N {
            return Err(Error::BadValue("hex digest length"));
        }
        let mut bytes = [0u8; N];
        for (dst, pair) in bytes.iter_mut().zip(hex.chunks_exact(2)) {
            *dst = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Digest { bytes })
    }

    /// Copies exactly `N` bytes out of `data`.
    ///
    /// A shorter slice is [`Error::Truncated`](`crate::Error::Truncated`)
    /// with `needed` set to `N`; a longer one is
    /// [`Error::BadValue`](`crate::Error::BadValue`) `"digest length"`.
    pub fn from_slice(data: &[u8]) -> Result<Self> {
        if data.len() < N {
            return Err(Error::truncated("digest", N, data.len()));
        }
        if data.len() > N {
            return Err(Error::BadValue("digest length"));
        }
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(data);
        Ok(Digest { bytes })
    }

    /// Reads exactly `N` bytes out of `stream`.
    ///
    /// Partial reads are followed until the digest is full. A source
    /// that supplies fewer than `N` bytes before it is exhausted yields
    /// [`Error::Truncated`](`crate::Error::Truncated`) with `found` set
    /// to the number of bytes actually supplied. A source with more than
    /// `N` bytes simply keeps the rest.
    ///
    /// ```
    /// let mut src: &[u8] = &[1, 2, 3];
    /// let d = modhash_primitives::Digest::<3>::from_stream(&mut src).unwrap();
    /// assert_eq!(d.as_bytes(), &[1, 2, 3]);
    /// ```
    pub fn from_stream<R: Read + ?Sized>(stream: &mut R) -> Result<Self> {
        let mut bytes = [0u8; N];
        let mut filled = 0usize;
        while filled < N {
            let n = stream.read_some(&mut bytes[filled..]);
            if n == 0 {
                return Err(Error::truncated("digest", N, filled));
            }
            filled += n;
        }
        Ok(Digest { bytes })
    }
}

impl<const N: usize> fmt::Display for Digest<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The same bytes `to_hex` produces, by construction: this impl
        // calls it, so the two paths cannot drift apart.
        let hex = self.to_hex();
        f.pad(core::str::from_utf8(&hex).map_err(|_| fmt::Error)?)
    }
}

/// Maps one ASCII byte to its nibble value.
fn nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::BadValue("hex digit")),
    }
}

/// The minimal byte source [`Digest::from_stream`] reads from.
///
/// The kit is `no_std` and `core`-only, so it cannot name
/// `std::io::Read`; this trait carries the one operation a digest
/// needs: *pull some bytes now, returning how many*. Host code adapts a
/// real reader in one line, e.g. for `std::io::Read`:
/// `fn read_some(&mut self, buf: &mut [u8]) -> usize { self.0.read(buf).unwrap_or(0) }`
/// — an I/O error surfaces as exhaustion, which `from_stream` reports
/// as [`Error::Truncated`](`crate::Error::Truncated`).
pub trait Read {
    /// Pulls as many bytes as the source can supply right now into
    /// `buf` and returns how many were written. Never writes more than
    /// `buf.len()`. Returning `0` signals that the source is exhausted.
    fn read_some(&mut self, buf: &mut [u8]) -> usize;
}

impl Read for &[u8] {
    fn read_some(&mut self, buf: &mut [u8]) -> usize {
        let n = self.len().min(buf.len());
        buf[..n].copy_from_slice(&self[..n]);
        *self = &self[n..];
        n
    }
}
