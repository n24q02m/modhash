//! The synthesis polyphase filterbank, ISO/IEC 11172-3 §2.4.3.2.
//!
//! Direct implementation of the spec's canonical flow:
//! `V` is a 1024-entry FIFO; each 32-subband block shifts it by 64, then
//! `U[i] = V[128 + 2i]` for `i < 32`, `U[32 + i] = V[96 + 2i]`; the output
//! sample is `Σ_j U[32j + i] · D[32j + i]` over `j = 0..15`, with `N` and
//! `D` from `tables` (`N` computed by formula, `D` the normative Annex B.3
//! constants cross-checked against jlayer's transposed copy).

use crate::tables::{D_WINDOW, N_MATRIX};

/// Polyphase filterbank state for one channel: the 1024-sample `V` FIFO and
/// the current write position. `Default` gives the all-zero state the spec
/// prescribes before the first frame.
#[derive(Clone)]
pub(crate) struct Synthesis {
    /// The V FIFO. Index 0 is the oldest byte; `v_off` tracks how much of
    /// the buffer is in use so steady state keeps 1024 entries.
    v: [f32; 1024],
}

impl Synthesis {
    /// A filterbank in its all-zero initial state.
    pub(crate) fn new() -> Self {
        Synthesis { v: [0.0; 1024] }
    }

    /// Push one block of 32 subband samples, return 32 PCM samples.
    ///
    /// Per spec the incoming 32 samples are matrixed into a fresh 64-entry
    /// segment at the head of `V`; older content shifts down by 64.
    pub(crate) fn process(&mut self, subband: &[f32; 32], out: &mut [f32; 32]) {
        // Shift V down by 64 and matrix the new block into the freed head.
        self.v.copy_within(0..960, 64);
        for i in 0..64 {
            let mut acc = 0.0f32;
            for (k, s) in subband.iter().enumerate() {
                acc += N_MATRIX[i * 32 + k] * s;
            }
            self.v[i] = acc;
        }
        // Build U by taking alternate 32-sample windows of V:
        // U[64i+j] = V[128i+j], U[64i+j+32] = V[128i+j+96].
        let mut u = [0.0f32; 512];
        for i in 0..8 {
            for j in 0..32 {
                u[64 * i + j] = self.v[128 * i + j];
                u[64 * i + j + 32] = self.v[128 * i + j + 96];
            }
        }
        for (o, out_s) in out.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for j in 0..16 {
                acc += u[32 * j + o] * D_WINDOW[32 * j + o];
            }
            *out_s = acc;
        }
    }
}

impl Default for Synthesis {
    fn default() -> Self {
        Self::new()
    }
}
