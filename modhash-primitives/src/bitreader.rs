//! A bit-level reader over a byte slice.
//!
//! The stream order is the ISO media convention (MP3, H.264, FLAC,
//! most length-prefixed containers): bit 0 of the stream is the
//! most-significant bit of byte 0, and a multi-bit field's first
//! stream bit becomes the result's most-significant bit. DEFLATE's
//! opposite packing lives behind `modhash-inflate`'s own reader, which
//! is RFC-specific by design.

use crate::error::{Error, Result};

/// Reads bit-level fields from a byte slice, never panicking.
///
/// All methods bound-check and return [`crate::Error::Truncated`]
/// instead of panicking or wrapping, so hostile input surfaces as an
/// error at the read that needed it.
#[derive(Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next bit to read, counting from the MSB of byte 0.
    pos: usize,
}

impl<'a> BitReader<'a> {
    /// Creates a reader over `data`, positioned at bit 0.
    pub const fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// The number of bits still readable.
    pub const fn remaining_bits(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    /// The current bit position from the start of the stream.
    pub const fn bit_position(&self) -> usize {
        self.pos
    }

    /// `true` when the position is on a byte boundary.
    pub const fn is_aligned(&self) -> bool {
        self.pos % 8 == 0
    }

    /// Discards bits up to the next byte boundary. Aligned readers are
    /// unchanged.
    pub fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// Reads `n` bits with the first stream bit becoming the
    /// most-significant bit of the result, for `n` in `0..=64`.
    ///
    /// ```
    /// use modhash_primitives::BitReader;
    ///
    /// let mut r = BitReader::new(&[0b1010_0110]);
    /// assert_eq!(r.bits(3).unwrap(), 0b101);
    /// assert_eq!(r.bits(5).unwrap(), 0b0_0110);
    /// ```
    pub fn bits(&mut self, n: usize) -> Result<u64> {
        if n > 64 {
            return Err(Error::BadValue("bit width over 64"));
        }
        if n > self.remaining_bits() {
            return Err(Error::truncated("bits", n, self.remaining_bits()));
        }
        let mut value = 0u64;
        for _ in 0..n {
            let bit = (self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1;
            value = (value << 1) | u64::from(bit);
            self.pos += 1;
        }
        Ok(value)
    }

    /// Reads one whole byte at the current (must be aligned) position.
    pub fn byte(&mut self) -> Result<u8> {
        if !self.is_aligned() {
            return Err(Error::BadValue("unaligned byte read"));
        }
        if self.remaining_bits() < 8 {
            return Err(Error::truncated("byte", 8, self.remaining_bits()));
        }
        let byte = self.data[self.pos / 8];
        self.pos += 8;
        Ok(byte)
    }
}
