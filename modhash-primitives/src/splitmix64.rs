//! A deterministic 64-bit PRNG (splitmix64) for tests and mixing.

/// The splitmix64 "golden gamma" increment.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// A splitmix64 generator (Steele, Lea & Flood 2014).
///
/// Carries its 64-bit state, advances by the golden gamma and runs
/// the xorshift-multiply output function. Two bytes of state for a
/// passing statistical battery is the reason this and not an LCG is
/// the shared PRNG: tests that need deterministic "random" data seed
/// it with a constant, never with the clock.
///
/// ```
/// let mut rng = modhash_primitives::SplitMix64::new(0);
/// assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
/// assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
/// ```
#[derive(Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a generator seeded with `seed`.
    pub const fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// Produces the next 64-bit output and advances the state.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Produces `n` pseudo-random bytes, filling `out` exactly.
    ///
    /// Deterministic in `out.len()` and the seed alone, so a test can
    /// regenerate the same corpus on any machine.
    pub fn fill_bytes(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let word = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}
