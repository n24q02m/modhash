//! BK-tree over 64-bit hashes under Hamming distance.
//!
//! A BK-tree (Burkhard–Keller tree) indexes a metric space so that a
//! radius query visits only the subtrees the triangle inequality cannot
//! rule out. The metric here is Hamming distance on the 64-bit perceptual
//! and audio fingerprints the kit produces — one XOR plus one
//! `count_ones`, a single `POPCNT`-class instruction per comparison.
//!
//! Design spec §4.4: the tree exists to produce *candidates* for the
//! full comparison in `match()`; it never decides "duplicate" by itself.
//!
//! # Layout and determinism
//!
//! Nodes live in an arena `Vec` and children are stored as
//! `(distance, arena index)` edges sorted by distance, so both insertion
//! and descent are iterative — attacker-sized inputs deepen the tree
//! without ever touching the call stack. Search pushes a stack on the
//! heap for the same reason.

use alloc::vec::Vec;

/// Hamming distance between two 64-bit hashes: one XOR, one popcount.
#[inline]
fn hamming64(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// One result of a [`BkTree::search`]: the stored id and its distance
/// from the query hash.
///
/// Results arrive sorted by `(distance, id)` — see
/// [`BkTree::search`]. `id` is `u64` so a caller can key with anything
/// from an ordinal to a content hash.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Candidate {
    /// The id passed to [`BkTree::insert`].
    pub id: u64,
    /// Hamming distance from the stored hash to the query hash.
    pub distance: u32,
}

/// One tree node in the arena: its hash, its caller's id, and the
/// outgoing edges `(edge distance, child arena index)` sorted by edge
/// distance.
#[derive(Debug)]
struct Node {
    hash: u64,
    id: u64,
    children: Vec<(u32, u32)>,
}

/// A BK-tree over `(u64 hash, u64 id)` entries under Hamming distance.
///
/// # Cost
///
/// `insert` is one descent, `search` visits only subtrees within
/// `radius` of the query edge distance. On uniform random hashes the
/// tree has expected depth `O(log n)` and a small-radius query is far
/// cheaper than the `O(n)` linear scan it replaces; on clustered hashes
/// the tree degenerates gracefully because distance-0 chains can be
/// entered but the prune bound still applies at every level.
#[derive(Debug)]
pub struct BkTree {
    nodes: Vec<Node>,
}

impl BkTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// The number of stored entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Stores `(hash, id)`; ids are opaque and duplicate ids or duplicate
    /// hashes are allowed — equal hashes chain down a distance-0 edge,
    /// which is where near-identical perceptual hashes land anyway.
    pub fn insert(&mut self, hash: u64, id: u64) {
        if self.nodes.is_empty() {
            self.nodes.push(Node {
                hash,
                id,
                children: Vec::new(),
            });
            return;
        }
        let mut cur = 0u32;
        loop {
            let d = hamming64(hash, self.nodes[cur as usize].hash);
            let edge = self.nodes[cur as usize]
                .children
                .binary_search_by_key(&d, |&(ed, _)| ed);
            match edge {
                Ok(i) => {
                    cur = self.nodes[cur as usize].children[i].1;
                }
                Err(i) => {
                    let idx = u32::try_from(self.nodes.len())
                        .expect("bk-tree holds at most u32::MAX nodes");
                    self.nodes.push(Node {
                        hash,
                        id,
                        children: Vec::new(),
                    });
                    self.nodes[cur as usize].children.insert(i, (d, idx));
                    return;
                }
            }
        }
    }

    /// Every stored entry within `radius` of `query`, sorted by
    /// `(distance, id)` so equal inputs always yield identical output.
    ///
    /// The prune is the BK-tree triangle-inequality bound: a node at
    /// distance `d` from the query has children keyed by the edge
    /// distance `e`, and a child subtree can hold a within-`radius`
    /// answer only when `|e − d| ≤ radius`. Iterative: the frontier lives
    /// in a heap `Vec`, so deep trees cannot overflow the call stack.
    #[must_use]
    pub fn search(&self, query: u64, radius: u32) -> Vec<Candidate> {
        let mut found = Vec::new();
        if self.nodes.is_empty() {
            return found;
        }
        let mut stack = Vec::from([0u32]);
        while let Some(idx) = stack.pop() {
            let node = &self.nodes[idx as usize];
            let d = hamming64(query, node.hash);
            if d <= radius {
                found.push(Candidate {
                    id: node.id,
                    distance: d,
                });
            }
            for &(e, child) in &node.children {
                // Triangle inequality: |e - d| <= radius, else the whole
                // subtree is out of range.
                if e.abs_diff(d) <= radius {
                    stack.push(child);
                }
            }
        }
        found.sort_unstable();
        found
    }
}

impl Default for BkTree {
    fn default() -> Self {
        Self::new()
    }
}
