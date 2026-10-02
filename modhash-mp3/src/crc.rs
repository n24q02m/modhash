//! The CRC-16 check protecting MPEG audio frames.
//!
//! ISO/IEC 11172-3 §2.4.1.3/§2.4.2.4: `crc_check` is the remainder of the
//! generator polynomial `X^16 + X^15 + X^2 + 1` (0x8005) applied MSB-first
//! with an all-ones initial state and no final XOR — the CRC-16/UMTS
//! parameterisation. Coverage differs per layer and is described where it
//! is used; this module only accumulates the checksum.
//!
//! The protected sections are not byte-aligned in Layer II (the scfsi
//! fields end mid-byte), so accumulation works one bit at a time; the
//! protected span is at most 200-odd bits per frame, so the bitwise loop
//! costs nothing measurable.

/// CRC-16/UMTS state, polynomial 0x8005, initial state 0xFFFF.
#[derive(Clone, Copy)]
pub(crate) struct Crc16(u16);

impl Crc16 {
    /// A fresh CRC accumulator in its all-ones initial state.
    pub(crate) fn new() -> Self {
        Crc16(0xFFFF)
    }
    /// Feed one bit (the next MSB of the protected stream).
    ///
    /// The update is the bitwise form of `crc ^= bit << 15; if top then
    /// crc = crc << 1 ^ poly`: the incoming bit XORs into the outgoing
    /// most-significant bit, deciding whether the shifted register takes
    /// the polynomial XOR. This equals the byte loop exactly.
    pub(crate) fn bit(&mut self, b: bool) {
        let fb = ((self.0 >> 15) as u8 ^ u8::from(b)) & 1;
        self.0 <<= 1;
        if fb != 0 {
            self.0 ^= 0x8005;
        }
    }

    /// Feed a whole byte, most significant bit first.
    pub(crate) fn byte(&mut self, b: u8) {
        for i in (0..8).rev() {
            self.bit((b >> i) & 1 != 0);
        }
    }

    /// Feed a byte slice.
    pub(crate) fn bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.byte(b);
        }
    }
    /// Feed a `n`-bit field just read MSB-first by a `BitReader` — the
    /// field's stream bits are exactly the bits of its value.
    pub(crate) fn field(&mut self, v: u64, n: usize) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 != 0);
        }
    }

    /// The running remainder; equal to the transmitted `crc_check` value
    /// when the covered bits were received intact.
    pub(crate) fn finish(self) -> u16 {
        self.0
    }
}
