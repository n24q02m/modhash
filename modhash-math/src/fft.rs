//! Complex and real discrete Fourier transforms over `f64`.
//!
//! The forward transform uses the unnormalized physics convention
//! `X[k] = Σ_j x[j]·e^(−2πi·j·k/N)` and the inverse undoes it exactly:
//! [`ifft`]`(`[`fft`]`(x)) = x` element-wise within a few ulps. All sizes
//! are powers of two; the kernel is the iterative radix-2 Cooley–Tukey
//! butterfly, in place, O(N log N) time, O(1) extra space.
//!
//! [`fft_real`] is the convenience wrapper the kit actually calls (audio
//! peak picking in `modhash-audio`): it packs the real input into a
//! complex buffer and runs the same kernel rather than the faster
//! real-input packing trick. The wasted factor of two is deliberate — the
//! pack/unpack split-radix path doubles the code for a saving that only
//! matters on inputs far larger than the kit's 4096-sample windows, and a
//! single butterfly is the path every test exercises.

use std::f64::consts::PI;

/// A complex number over `f64`, stored as two contiguous scalars.
///
/// The kit keeps its own type rather than taking a dependency on `num`
/// or `rustfft` — the workspace rule is zero external crates — and a
/// two-field record is all the FFT kernel needs.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// The zero value.
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };

    /// Constructs `re + i·im`.
    #[must_use]
    #[inline]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// `e^(i·θ)` — the unit-magnitude twiddle factor builder.
    ///
    /// Computed from a fresh `cos`/`sin` pair every call, never by
    /// repeated multiplication: twiddles accumulated by
    /// `w = w * wlen` drift measurably at N = 4096, while a direct
    /// `cis` call keeps the error at a few ulps per factor.
    #[must_use]
    #[inline]
    pub fn cis(theta: f64) -> Self {
        Self {
            re: theta.cos(),
            im: theta.sin(),
        }
    }

    /// Complex conjugate `re − i·im`.
    #[must_use]
    #[inline]
    pub const fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// `|z|²` — cheaper than [`norm`](Self::norm), and the quantity
    /// Parseval's identity and the audio peak picker actually compare.
    #[must_use]
    #[inline]
    pub fn norm_sq(self) -> f64 {
        self.re.mul_add(self.re, self.im * self.im)
    }

    /// `|z|`. Uses [`f64::hypot`] so a huge component does not overflow
    /// the squared intermediate.
    #[must_use]
    #[inline]
    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Multiplies every part by the real scalar `s`.
    #[must_use]
    #[inline]
    pub const fn scale(self, s: f64) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }
}

impl std::ops::Add for Complex {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            re: self.re + rhs.re,
            im: self.im + rhs.im,
        }
    }
}

impl std::ops::Sub for Complex {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            re: self.re - rhs.re,
            im: self.im - rhs.im,
        }
    }
}

impl std::ops::Mul for Complex {
    type Output = Self;

    /// `(re + i·im)·(rhs.re + i·rhs.im)`; one `mul_add` per component
    /// keeps the rounding tight.
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self {
            re: self.re.mul_add(rhs.re, -(self.im * rhs.im)),
            im: self.re.mul_add(rhs.im, self.im * rhs.re),
        }
    }
}

impl std::ops::Neg for Complex {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        Self {
            re: -self.re,
            im: -self.im,
        }
    }
}
/// In-place forward DFT: `X[k] = Σ_j x[j]·e^(−2πi·j·k/N)`.
///
/// Iterative radix-2 Cooley–Tukey: the buffer is permuted into
/// bit-reversed order once, then butterflies combine in `log₂N` passes.
/// Twiddle factors are computed directly with [`Complex::cis`] rather
/// than accumulated by multiplication, so accuracy does not depend on
/// N (see the `cis` note).
///
/// # Panics
///
/// Panics if `buf.len()` is not a power of two (zero included). The kit
/// only transforms windows it sized itself, so a violated precondition
/// is a caller bug, not input data.
pub fn fft(buf: &mut [Complex]) {
    let n = buf.len();
    assert!(n.is_power_of_two(), "fft length must be a power of two");
    if n <= 1 {
        return;
    }
    bit_reverse_permute(buf);
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        // Primitive `len`-th root of unity, e^(−2πi/len). One `cis`
        // per stage, then a fresh `cis` per j so rounding never stacks.
        for chunk in buf.chunks_exact_mut(len) {
            for j in 0..half {
                let w = Complex::cis(-2.0 * PI * (j as f64) / (len as f64));
                let u = chunk[j];
                let v = chunk[j + half] * w;
                chunk[j] = u + v;
                chunk[j + half] = u - v;
            }
        }
        len *= 2;
    }
}

/// In-place inverse DFT: `x[j] = (1/N)·Σ_k X[k]·e^(+2πi·j·k/N)`.
///
/// Implemented as `conj(fft(conj(X)))/N`, the standard trick that keeps
/// a single butterfly kernel: conjugating maps the forward sign
/// convention onto the inverse one, so [`fft`] is the only code path
/// that needs to be correct.
///
/// # Panics
///
/// Same precondition as [`fft`]: `buf.len()` must be a power of two.
pub fn ifft(buf: &mut [Complex]) {
    let n = buf.len();
    assert!(n.is_power_of_two(), "ifft length must be a power of two");
    if n <= 1 {
        return;
    }
    for x in buf.iter_mut() {
        *x = x.conj();
    }
    fft(buf);
    let inv = 1.0 / (n as f64);
    for x in buf.iter_mut() {
        *x = x.conj().scale(inv);
    }
}

/// Forward DFT of a real signal, returned as `N` complex bins.
///
/// Bins above `N/2` are the conjugate mirror of the lower half — a real
/// input carries `N/2 + 1` independent bins — but the full vector is
/// returned so callers can index by the same convention as [`fft`].
/// Power-of-two length required, same as [`fft`]; `N = 0` returns an
/// empty vector.
///
/// Allocates one `Vec` of `N` complexes. This is the real-input
/// convenience path documented in the module note: clear and
/// single-kernel, at the cost of doing a complex transform's work on
/// real data.
///
/// # Panics
///
/// Panics if `input.len()` is not a power of two.
#[must_use]
pub fn fft_real(input: &[f64]) -> Vec<Complex> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut buf: Vec<Complex> = input.iter().map(|&re| Complex::new(re, 0.0)).collect();
    fft(&mut buf);
    buf
}

/// Reorders `buf` so element `i` lands at `bit-reverse(i)` within
/// `log₂(buf.len())` bits — the permutation the iterative kernel needs
/// before its first butterfly pass.
fn bit_reverse_permute(buf: &mut [Complex]) {
    let n = buf.len();
    // j walks the bit-reversed counterpart of each i. The classic
    // incremental update avoids a per-element bit reversal: flipping
    // the highest set bit then carrying down is O(1) amortized.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }
}
