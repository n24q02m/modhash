//! The inverted `peak → (t, id)` table and Δt-histogram matching.
//!
//! Semantics pinned in `docs/algorithms/audio-peaks.md`: a query peak
//! `(t_q, f)` casts one vote at `Δt = t_q − t_stored` for every stored
//! `(t_stored, id)` under key `f`. Positive Δt therefore means the
//! query content plays *later* than its indexed copy — a 2 s query
//! prefix lands at Δt ≈ +43 frames. An id's modal Δt is its
//! most-voted offset (ties → smallest Δt); `votes` counts the
//! histogram mass at `modal Δt ± DELTA_TOL`.
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::Signature;
use crate::peaks::SLOT_COUNT;

/// Histogram tolerance in frames: votes at `modal Δt ± 1` count toward
/// the same match, covering the ±46.4 ms frame-boundary rounding of an
/// arbitrary sample offset.
pub const DELTA_TOL: i32 = 1;

/// Inverted index over a corpus of signatures: slot → sorted list of
/// `(stored frame, corpus id)`. `id` is the signature's position in
/// the `build_index` argument.
#[derive(Clone, Debug)]
pub struct Index {
    /// `table[f]` = every `(t, id)` whose signature carries a peak in
    /// slot `f`, sorted for deterministic iteration.
    table: [Vec<(u32, u32)>; SLOT_COUNT],
}

/// One corpus member's hit: the modal time offset and its vote mass.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Position of the matched signature in the `build_index` input.
    pub id: u32,
    /// Modal `query_t − stored_t` in frames (`HOP` = 2048 samples each,
    /// ≈ 46.4 ms). Positive means the query content starts later —
    /// a 2 s prefix lands at ≈ +43.
    pub delta_t: i64,
    /// Votes within `delta_t ± DELTA_TOL` — the histogram peak mass.
    pub votes: u32,
    /// Total raw votes the id received across all offsets, before the
    /// tolerance window — diagnostic for diluted matches.
    pub total_votes: u32,
}

/// Builds the inverted table over `sigs`: `table[f]` collects
/// `(peak.t, id)` for every peak of every signature.
#[must_use]
pub fn build_index(sigs: &[Signature]) -> Index {
    let table: [Vec<(u32, u32)>; SLOT_COUNT] = core::array::from_fn(|_| Vec::new());
    let mut index = Index { table };
    for (id, sig) in sigs.iter().enumerate() {
        for peak in sig.peaks() {
            index.table[usize::from(peak.f)].push((peak.t, id as u32));
        }
    }
    for entry in &mut index.table {
        entry.sort_unstable();
    }
    index
}

/// Matches `query` against `index`, returning one [`Match`] per corpus
/// id that shares at least one peak slot, best first: `votes`
/// descending, `id` ascending — a total order, so output is
/// deterministic.
#[must_use]
pub fn match_signature(query: &Signature, index: &Index) -> Vec<Match> {
    // id -> Δt histogram.
    let mut hist: BTreeMap<u32, BTreeMap<i64, u32>> = BTreeMap::new();
    for qp in query.peaks() {
        for &(t, id) in &index.table[usize::from(qp.f)] {
            let delta = i64::from(qp.t) - i64::from(t);
            *hist.entry(id).or_default().entry(delta).or_insert(0) += 1;
        }
    }
    let mut out = Vec::with_capacity(hist.len());
    for (id, deltas) in hist {
        // Modal Δt: most votes; ties resolve to the smallest offset so
        // two corpus copies of the same content cannot flip order.
        let (&modal, _) = deltas
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .expect("id in map has at least one vote");
        let votes: u32 = deltas
            .range(modal - i64::from(DELTA_TOL)..=modal + i64::from(DELTA_TOL))
            .map(|(_, &v)| v)
            .sum();
        let total_votes: u32 = deltas.values().sum();
        out.push(Match {
            id,
            delta_t: modal,
            votes,
            total_votes,
        });
    }
    out.sort_by(|a, b| b.votes.cmp(&a.votes).then(a.id.cmp(&b.id)));
    out
}
