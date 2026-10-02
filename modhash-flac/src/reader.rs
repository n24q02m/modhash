//! FLAC-specific reads on top of [`modhash_primitives::BitReader`].
//!
//! The primitive supplies the MSB-first cursor and the bound-checked core
//! `bits` read; this wrapper adds what FLAC needs on top of it: named
//! [`Error::Truncated`] causes (the primitive reports every truncation as
//! `"bits"`, which cannot say *what* ran short), two's-complement signed
//! fields, unary coding, and the zero-pad alignment check the format
//! mandates before a frame's CRC-16.

use modhash_primitives::{BitReader, Error, Result};

/// A [`BitReader`] that also carries the backing slice for CRC spans and
/// reports truncation under the name of the field being read.
pub(crate) struct FlacReader<'a> {
    r: BitReader<'a>,
    data: &'a [u8],
}

impl<'a> FlacReader<'a> {
    /// Creates a reader at bit 0 of `data`.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        FlacReader {
            r: BitReader::new(data),
            data,
        }
    }

    /// The whole slice under the reader. The frame CRCs cover spans of it.
    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Current position in bits from the start of the input.
    pub(crate) fn pos(&self) -> usize {
        self.r.bit_position()
    }

    /// Bits remaining, `len * 8 - pos`. Never negative.
    pub(crate) fn remaining(&self) -> usize {
        self.r.remaining_bits()
    }

    /// Reads `n` bits MSB-first into a `u64`, for `n` in `0..=64`.
    /// Truncation is reported under `what` instead of the primitive's
    /// generic `"bits"`, so a short input names the field it died in.
    pub(crate) fn bits(&mut self, n: u32, what: &'static str) -> Result<u64> {
        if n as usize > self.r.remaining_bits() {
            return Err(Error::truncated(
                what,
                (self.r.bit_position() + n as usize).div_ceil(8),
                self.data.len(),
            ));
        }
        self.r.bits(n as usize)
    }

    /// Reads one bit as a `u32` 0 or 1.
    pub(crate) fn bit(&mut self, what: &'static str) -> Result<u32> {
        Ok(self.bits(1, what)? as u32)
    }

    /// Reads `n` bits as a two's-complement signed value, sign-extended to
    /// `i64`. FLAC warmup samples, verbatim samples and escaped Rice
    /// residuals are signed fields. `n` must be in `0..=64`.
    pub(crate) fn signed(&mut self, n: u32, what: &'static str) -> Result<i64> {
        let v = self.bits(n, what)?;
        if n == 0 || n >= 64 {
            return Ok(v as i64);
        }
        let sign = (v >> (n - 1)) & 1;
        Ok(if sign == 1 {
            (v | ((!0u64) << n)) as i64
        } else {
            v as i64
        })
    }

    /// Reads a unary-coded value: the count of `0` bits before the next
    /// `1` bit, per RFC 9639's Rice and wasted-bits coding (`q` zero bits
    /// then a one bit represent `q`). The FLAC format document's own
    /// prose contradicts itself on the bit polarity; the tables, libFLAC
    /// and every conforming file use 0-then-1.
    pub(crate) fn unary(&mut self, what: &'static str) -> Result<u32> {
        let mut q = 0u32;
        loop {
            match self.bit(what)? {
                0 => q = q.saturating_add(1),
                _ => return Ok(q),
            }
        }
    }

    /// Advances to the next byte boundary, requiring every skipped
    /// padding bit to be zero (RFC 9639 mandates zero padding between
    /// the last subframe and the frame's CRC-16). A non-zero pad bit is
    /// corruption, so it is [`Error::BadValue`], not a silent skip.
    pub(crate) fn align_zero(&mut self, what: &'static str) -> Result<()> {
        while self.pos() % 8 != 0 {
            if self.bit(what)? != 0 {
                return Err(Error::BadValue(what));
            }
        }
        Ok(())
    }
}
