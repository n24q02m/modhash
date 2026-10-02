//! `buzhash64`: a classic buzhash-style rolling hash over a fixed 64-byte
//! sliding window, distinct from the Gear fingerprint the chunker runs on.
//!
//! Where Gear's `fp = (fp << 1) + GEAR[b]` lets old bytes shift off the end
//! of the word on their own, buzhash keeps an explicit window and evicts the
//! oldest byte's contribution with an XOR. With a 64-byte window the evicted
//! byte's `GEAR` constant has been rotated by `64 mod 64 == 0` extra bits,
//! so eviction is just `fp ^= GEAR[oldest]` — the whole point of picking a
//! window whose size equals the fingerprint width.
//!
//! The hash of a full window `w[0..64]` is the folded value
//! `GEAR[w[0]] <<< 63 ^ GEAR[w[1]] <<< 62 ^ ... ^ GEAR[w[63]]`, i.e. each
//! byte's constant rotated by the number of subsequent bytes. Pushing a byte
//! into a full window therefore rolls the state forward exactly as if the
//! last 64 bytes had been re-hashed from scratch; `tests/` proves that
//! equivalence directly.

use crate::gear::GEAR;

/// The sliding-window width of [`Buzhash64`]: 64 bytes, one rotation cycle
/// of the 64-bit fingerprint.
pub const BUZHASH_WINDOW: usize = 64;

/// A buzhash rolling hash over the last [`BUZHASH_WINDOW`] bytes pushed.
///
/// ```
/// use modhash_fastcdc::{BUZHASH_WINDOW, Buzhash64};
///
/// let mut h = Buzhash64::new();
/// for &b in b"hello world" {
///     h.push(b);
/// }
/// assert_eq!(h.len(), 11); // window is not full yet
/// ```
#[derive(Clone, Debug)]
pub struct Buzhash64 {
    window: [u8; BUZHASH_WINDOW],
    /// Index the next byte is written to (and the oldest byte lives at once
    /// the window is full).
    pos: usize,
    /// Number of bytes currently in the window, saturating at 64.
    len: usize,
    fp: u64,
}

impl Default for Buzhash64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Buzhash64 {
    /// An empty rolling window.
    pub const fn new() -> Self {
        Buzhash64 {
            window: [0; BUZHASH_WINDOW],
            pos: 0,
            len: 0,
            fp: 0,
        }
    }

    /// The current fingerprint of the window contents.
    pub const fn fingerprint(&self) -> u64 {
        self.fp
    }

    /// Number of bytes currently in the window, `0..=64`.
    pub const fn len(&self) -> usize {
        self.len
    }
    /// `true` while the window holds fewer than 64 bytes.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` once 64 bytes have been pushed without a [`reset`](Self::reset).
    pub const fn is_full(&self) -> bool {
        self.len == BUZHASH_WINDOW
    }

    /// Empties the window and zeroes the fingerprint.
    pub fn reset(&mut self) {
        self.pos = 0;
        self.len = 0;
        self.fp = 0;
    }

    /// Pushes `byte` into the window, evicting the oldest byte once the
    /// window is full, and returns the new fingerprint.
    ///
    /// The update is `fp = (fp <<< 1) ^ GEAR[b]` followed by
    /// `fp ^= GEAR[evicted]` when a byte falls out; rotation by 64 is the
    /// identity, so the evicted byte's contribution is removed un-rotated.
    #[inline]
    pub fn push(&mut self, byte: u8) -> u64 {
        self.fp = self.fp.rotate_left(1) ^ GEAR[byte as usize];
        if self.is_full() {
            self.fp ^= GEAR[self.window[self.pos] as usize];
        } else {
            self.len += 1;
        }
        self.window[self.pos] = byte;
        self.pos = (self.pos + 1) % BUZHASH_WINDOW;
        self.fp
    }

    /// Pushes every byte of `data` in order and returns the fingerprint.
    pub fn update(&mut self, data: &[u8]) -> u64 {
        for &b in data {
            self.push(b);
        }
        self.fp
    }
}
