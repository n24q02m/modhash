//! Checksums and non-cryptographic 64-bit hashes: CRC-32 (reflected,
//! the PNG/zlib polynomial), Adler-32 (RFC 1950) and FNV-1a 64.

/// The reflected CRC-32 polynomial 0xEDB88320 (the PNG/zlib one).
const CRC32_POLY: u32 = 0xEDB8_8320;

/// The 256-entry processing table, built by the compiler at compile
/// time so a call costs one lookup per byte and nothing else.
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                CRC32_POLY ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// Computes the reflected CRC-32 of `data` (PNG spec / ISO 3309).
///
/// ```
/// assert_eq!(modhash_primitives::crc32(b"123456789"), 0xCBF4_3926);
/// ```
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = CRC32_TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Computes the Adler-32 checksum of `data` (RFC 1950 8.2).
///
/// The modulo is applied every 5552 bytes (NMAX): 5552 is the largest
/// run over which the sum cannot overflow a u32, so the result is
/// identical to applying `% 65521` per byte while touching the
/// modulo a few thousand times less.
///
/// ```
/// assert_eq!(modhash_primitives::adler32(b"Wikipedia"), 0x11E6_0398);
/// ```
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    const NMAX: usize = 5552;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(NMAX) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// The FNV-1a 64-bit offset basis.
const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// The FNV-1a 64-bit prime.
const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Computes the 64-bit FNV-1a hash of `data`.
///
/// ```
/// assert_eq!(modhash_primitives::fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
/// ```
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash = FNV1A64_OFFSET;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV1A64_PRIME);
    }
    hash
}
