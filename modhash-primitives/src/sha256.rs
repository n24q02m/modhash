//! FIPS 180-2 SHA-256.
//!
//! One `compress` step over the 64 round constants, invoked per 64-byte
//! block. The message schedule lives in a fixed `[u32; 64]`, so hostile
//! input lengths cannot drive an allocation.

use crate::error::Result;

/// The SHA-256 initial hash values (FIPS 180-2 5.3.3).
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// The 64 round constants (FIPS 180-2 4.2.2), the fractional parts of the
/// cube roots of the first 64 primes.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The 64-bit length suffix is appended big-endian, so the message length in
/// bits must fit: the FIPS padding shape cannot express a longer message.
const MAX_INPUT_BYTES: u64 = (u64::MAX >> 3) - 71;

/// Computes the SHA-256 digest of `data` (FIPS 180-2).
///
/// # Errors
///
/// [`crate::Error::TooLarge`] if `data` is longer than the padding format
/// can express.
pub fn sha256(data: &[u8]) -> Result<crate::Digest<32>> {
    if data.len() as u64 > MAX_INPUT_BYTES {
        return Err(crate::Error::too_large("sha256 input", usize::MAX));
    }

    let mut h = H0;
    let mut chunks = data.chunks_exact(64);
    for block in chunks.by_ref() {
        compress(&mut h, block.try_into().expect("chunks_exact yields 64"));
    }

    // Padding: 0x80, zeros, then the 64-bit big-endian bit length, filling
    // one final block. A message that leaves fewer than 9 bytes takes a
    // second block, which the loop over full empty blocks handles naturally.
    let rem = chunks.remainder();
    let bit_len = (data.len() as u64) << 3;
    let mut tail = [0u8; 128];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    let pad_len = if rem.len() + 9 <= 64 { 64 } else { 128 };
    tail[pad_len - 8..pad_len].copy_from_slice(&bit_len.to_be_bytes());
    for block in tail[..pad_len].chunks_exact(64) {
        compress(&mut h, block.try_into().expect("chunks_exact yields 64"));
    }

    let mut out = [0u8; 32];
    for (word, chunk) in h.iter().zip(out.chunks_exact_mut(4)) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    Ok(crate::Digest::from_bytes(out))
}

/// One compression step: 64 rounds over the message schedule for `block`.
fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (word, chunk) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes(chunk.try_into().expect("chunks_exact yields 4"));
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *slot = slot.wrapping_add(value);
    }
}
