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

#[cfg(test)]
mod tests {
    use super::Crc16;

    /// ISO/IEC 11172-3 frame CRC: poly 0x8005, init 0xFFFF, MSB-first.
    /// `"123456789"` → 0xAEE7 — the oft-quoted catalog value 0xFEE8 is
    /// CRC-16/BUYPASS, which shares the polynomial but inits to 0x0000;
    /// this pin is the MPEG-initialised variant.
    #[test]
    fn known_answer_123456789() {
        let mut c = Crc16::new();
        c.bytes(b"123456789");
        assert_eq!(c.finish(), 0xAEE7);
    }

    #[test]
    fn empty_is_init_state() {
        assert_eq!(Crc16::new().finish(), 0xFFFF);
    }

    /// Bit-by-bit feeding must equal the byte path exactly (L2 scfsi
    /// coverage ends mid-byte).
    #[test]
    fn bitwise_matches_bytewise() {
        let mut a = Crc16::new();
        a.bytes(&[0xA5, 0x3C]);
        let mut b = Crc16::new();
        for byte in [0xA5u8, 0x3C] {
            for i in (0..8).rev() {
                b.bit((byte >> i) & 1 != 0);
            }
        }
        assert_eq!(a.finish(), b.finish());
    }

    /// A mid-byte field feed covers exactly the field's stream bits.
    #[test]
    fn field_feeds_msb_first() {
        let mut a = Crc16::new();
        a.field(0b1011, 4);
        let mut b = Crc16::new();
        for bit in [true, false, true, true] {
            b.bit(bit);
        }
        assert_eq!(a.finish(), b.finish());
    }
}
