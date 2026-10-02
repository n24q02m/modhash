//! Threshold calibration on labelled pairs.
//!
//! Design spec §4.4: a threshold is a **measurement on the user's own
//! data**, not a constant shipped in the crate. [`calibrate`] takes a
//! set of `(distance, is_match)` observations and returns the largest
//! distance that maximizes the profile's F-beta score, together with
//! the counts behind it, so the chosen threshold always carries its own
//! receipt.
//!
//! Two profiles exist because the cost of each error direction is not
//! symmetric:
//!
//! * [`Profile::Dedup`] — a false merge destroys an original the user
//!   cannot get back, while a missed duplicate only wastes storage.
//!   "Rather miss than over-merge" ⇒ precision-weighted **F0.5**, and
//!   among equal scores the *fewest* predicted matches.
//! * [`Profile::Search`] — a missed hit is a lost answer, while a false
//!   alarm only costs one `match()` call that rejects it. "Rather flag
//!   than miss" ⇒ recall-weighted **F2**, and among equal scores the
//!   *most* predicted matches.
//!
//! # Reproducibility
//!
//! The score is computed on counts, not floating accumulated weights,
//! every boundary is decided by integer comparisons, and the tie-break
//! is part of the contract: calling [`calibrate`] twice on the same
//! pairs returns the same [`Calibration`] — the plan's "running it
//! twice must give the same result" requirement.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

/// Which error direction is more expensive; selects the F-beta weight
/// and the tie-break direction in [`calibrate`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Deduplication: a false positive deletes a keeper.
    ///
    /// Maximizes F0.5 (precision counted twice), ties resolved toward
    /// the *smaller* prediction set.
    Dedup,
    /// Similarity search: a false negative loses the answer.
    ///
    /// Maximizes F2 (recall counted twice), ties resolved toward the
    /// *larger* prediction set.
    Search,
}

impl Profile {
    /// The β of the profile's F-beta score.
    fn beta(self) -> f64 {
        match self {
            Profile::Dedup => 0.5,
            Profile::Search => 2.0,
        }
    }

    /// Whether a larger prediction set wins a score tie (`Search`) or a
    /// smaller one (`Dedup`).
    fn prefers_recall(self) -> bool {
        matches!(self, Profile::Search)
    }
}

/// The threshold [`calibrate`] selected and the counts that justify it.
///
/// `threshold` is the largest distance classified as a match: `dist <=
/// threshold` ⇒ match. [`None`] means the best score was achieved by
/// matching nothing — which only happens when the set holds no positive
/// pairs or every boundary scores zero; the counts still describe that
/// degenerate optimum honestly.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Calibration {
    /// Largest distance classified "match"; [`None`] = match nothing.
    pub threshold: Option<u32>,
    /// F-beta score of the selected boundary.
    pub score: f64,
    /// Positive pairs at or below the threshold.
    pub true_positives: usize,
    /// Negative pairs at or below the threshold.
    pub false_positives: usize,
    /// Positive pairs above the threshold.
    pub false_negatives: usize,
    /// Negative pairs above the threshold.
    pub true_negatives: usize,
}

impl Calibration {
    /// Precision of the selected boundary: `tp / (tp + fp)`, 0 when the
    /// boundary predicts nothing.
    #[must_use]
    pub fn precision(&self) -> f64 {
        let predicted = self.true_positives + self.false_positives;
        if predicted == 0 {
            0.0
        } else {
            self.true_positives as f64 / predicted as f64
        }
    }

    /// Recall of the selected boundary: `tp / (tp + fn)`, 0 when the
    /// set holds no positive pairs.
    #[must_use]
    pub fn recall(&self) -> f64 {
        let positives = self.true_positives + self.false_negatives;
        if positives == 0 {
            0.0
        } else {
            self.true_positives as f64 / positives as f64
        }
    }
}

/// One candidate boundary's score and the counts behind it; compared
/// lexicographically with the predicted-count sign chosen by the
/// profile.
struct Scored {
    threshold: Option<u32>,
    score: f64,
    tp: usize,
    fp: usize,
    positives: usize,
    negatives: usize,
}

impl Scored {
    /// `true` when `self` beats `other` under `profile`: higher score
    /// first, then the profile's prediction-count direction.
    fn beats(&self, other: &Self, profile: Profile) -> bool {
        if self.score != other.score {
            return self.score > other.score;
        }
        let mine = self.tp + self.fp;
        let theirs = other.tp + other.fp;
        if profile.prefers_recall() {
            mine > theirs
        } else {
            mine < theirs
        }
    }

    /// F-beta of this boundary; 0 when it catches no positive — both
    /// precision and recall are 0 there, so the ratio is undefined and
    /// must not be left to a 0/0.
    fn f_beta(&self, beta: f64) -> f64 {
        if self.tp == 0 || self.positives == 0 {
            return 0.0;
        }
        let predicted = self.tp + self.fp;
        let precision = self.tp as f64 / predicted as f64;
        let recall = self.tp as f64 / self.positives as f64;
        let b2 = beta * beta;
        (1.0 + b2) * precision * recall / (b2 * precision + recall)
    }

    fn into_calibration(self) -> Calibration {
        Calibration {
            threshold: self.threshold,
            score: self.score,
            true_positives: self.tp,
            false_positives: self.fp,
            false_negatives: self.positives - self.tp,
            true_negatives: self.negatives - self.fp,
        }
    }
}

/// Chooses the matching threshold on a labelled set of pairs.
///
/// `pairs` is a slice of `(distance, is_match)` observations — distance
/// from whatever comparison produced the pairs, `true` when the pair is
/// a real match. The predicate `distance <= threshold` ⇒ "match" is
/// evaluated at every boundary the data can distinguish: each distinct
/// observed distance, plus the "match nothing" boundary. The boundary
/// maximizing the profile's F-beta wins, with the deterministic
/// tie-break documented on [`Profile`].
///
/// The threshold is returned as the smallest observed distance at the
/// winning boundary (equivalent distances collapse to their minimum),
/// so the receipt is always a number that appeared in the input.
///
/// # Errors
///
/// [`Error::BadValue`] on an empty `pairs` slice: a threshold measured
/// on nothing is not a measurement.
///
/// # Complexity
///
/// `O(n log n)` for the distance sort plus one `O(n)` boundary sweep —
/// not the naive `O(n²)` rescan.
pub fn calibrate(pairs: &[(u32, bool)], profile: Profile) -> Result<Calibration> {
    if pairs.is_empty() {
        return Err(Error::BadValue("calibration pairs"));
    }
    let positives = pairs.iter().filter(|p| p.1).count();
    let negatives = pairs.len() - positives;
    if positives == 0 {
        // No positive pairs: every boundary scores 0 and the honest
        // answer is "match nothing", whatever the tie-break says.
        return Ok(Calibration {
            threshold: None,
            score: 0.0,
            true_positives: 0,
            false_positives: 0,
            false_negatives: 0,
            true_negatives: negatives,
        });
    }

    // tp/fp at each candidate threshold, walked in one pass over the
    // distance-sorted pairs instead of rescanning them per boundary.
    let mut sorted_pairs: Vec<(u32, bool)> = pairs.to_vec();
    sorted_pairs.sort_unstable_by_key(|p| p.0);

    let beta = profile.beta();
    // The "match nothing" boundary, evaluated through f_beta like every
    // other candidate so its score field is computed, not assumed.
    let mut best = Scored {
        threshold: None,
        score: 0.0,
        tp: 0,
        fp: 0,
        positives,
        negatives,
    };
    best.score = best.f_beta(beta);

    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut i = 0usize;
    while i < sorted_pairs.len() {
        let d = sorted_pairs[i].0;
        while i < sorted_pairs.len() && sorted_pairs[i].0 == d {
            if sorted_pairs[i].1 {
                tp += 1;
            } else {
                fp += 1;
            }
            i += 1;
        }
        let mut cand = Scored {
            threshold: Some(d),
            score: 0.0,
            tp,
            fp,
            positives,
            negatives,
        };
        cand.score = cand.f_beta(beta);
        if cand.beats(&best, profile) {
            best = cand;
        }
    }

    Ok(best.into_calibration())
}
