//! Reciprocal Rank Fusion (RRF) for hybrid code search.
//!
//! Fuses the independently-ranked lanes of [`crate::search`] — vector (semantic), keyword (BM25),
//! and exact (symbol) — into one ranking without needing their scores to be comparable. Each lane
//! contributes `weight / (k + rank)` to every chunk it ranks (rank is 1-based), and a chunk's fused
//! score is the sum across lanes. RRF is score-scale-agnostic (it only reads ranks), which is why it
//! can blend an L2 distance, a BM25 score, and a symbol match order without normalization.
//!
//! The universal join key is `chunk_id` (`<source-hash-hex>:<ordinal>`), emitted by every lane.

use std::cmp::Ordering;

use ahash::{AHashMap, AHashSet};

/// The RRF rank-damping constant. 60 is the value from the original Cormack et al. paper and the
/// de-facto default across search stacks — large enough that the top few ranks of each lane stay
/// close in contribution, so no single lane dominates on rank-1 alone.
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Stable lane names, surfaced as per-hit `matched_lanes` provenance in `search_code` responses.
pub const LANE_EXACT: &str = "exact";
/// Vector (semantic) lane name.
pub const LANE_VECTOR: &str = "vector";
/// Keyword (BM25) lane name.
pub const LANE_KEYWORD: &str = "keyword";
/// Facet lane name. A facet lane prefilters without scoring, so it contributes a rank over the
/// surviving set rather than a scored match — the documents tier fuses it like any other lane.
pub const LANE_FACET: &str = "facet";

/// Per-lane weights for [`rrf_fuse_detailed`].
///
/// The defaults are the code tier's current values, unchanged, so code-search ranking is exactly
/// what it was before this struct existed. They are deliberately **not** the documents tier's
/// spec §5 values: those are a separate, provisional set that the evaluation harness has yet to
/// justify, and swapping them in here would silently change the tier that ships today. Construct
/// [`FusionWeights::document_lanes`] explicitly when you want that set.
///
/// Note there is another weights struct in the tree — `src/pii.rs`'s `RrfWeights`, holding
/// 3.0/2.0/1.0, which are the §5 values. It has no production reader. If you are here looking for
/// "the weights struct", that is why you found it, and adopting its values will change code-search
/// ranking and break `search_code_hybrid_ranks_exact_symbol_first`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionWeights {
    /// Exact / identifier lane.
    pub exact: f32,
    /// Vector (semantic) lane.
    pub vector: f32,
    /// Keyword (BM25 / lexical) lane.
    pub keyword: f32,
    /// Facet prefilter lane — rank over the surviving set, not a scored match.
    pub facet: f32,
}

impl FusionWeights {
    /// The code tier's shipped weights. Exact is highest because an identifier-shaped query that
    /// matches a defined symbol is high-precision: the chunk that *defines* the symbol should win
    /// ties against a lexical or semantic co-occurrence.
    pub const fn code_lanes() -> Self {
        Self {
            exact: 2.0,
            vector: 1.0,
            keyword: 1.0,
            facet: 0.0,
        }
    }

    /// Spec 0012 §5's document-tier weights. **Provisional** — the evaluation harness has not run,
    /// so these are the direction of the argument, not a measurement. `facet: 1.0` is a rank over
    /// the prefiltered set, not a score; the facet lane prefilters without scoring.
    pub const fn document_lanes() -> Self {
        Self {
            exact: 3.0,
            keyword: 2.0,
            facet: 1.0,
            vector: 1.0,
        }
    }

    /// The weight for a lane by name. Unknown lanes weigh 0, which makes them inert in the fusion
    /// rather than a silent panic — a tier that names a lane this build does not know about loses
    /// that lane instead of the whole query.
    pub fn for_lane(&self, name: &str) -> f32 {
        match name {
            LANE_EXACT => self.exact,
            LANE_VECTOR => self.vector,
            LANE_KEYWORD => self.keyword,
            LANE_FACET => self.facet,
            _ => 0.0,
        }
    }
}

impl Default for FusionWeights {
    fn default() -> Self {
        Self::code_lanes()
    }
}

/// One ranked lane's contribution to the fusion: its name, chunk ids (best-first), and weight.
pub struct FusionLane<'a> {
    /// Stable lane identity (`"exact"` / `"vector"` / `"keyword"`) — echoed in per-hit provenance.
    pub name: &'static str,
    /// Chunk ids in rank order, best first. Duplicates within a lane are ignored after the first.
    pub chunk_ids: &'a [String],
    /// Lane weight — scales this lane's `1 / (k + rank)` contribution.
    pub weight: f32,
}

impl<'a> FusionLane<'a> {
    /// Construct a lane from its name, a ranked slice, and a weight.
    pub fn new(name: &'static str, chunk_ids: &'a [String], weight: f32) -> Self {
        Self {
            name,
            chunk_ids,
            weight,
        }
    }
}

/// A fused hit with per-lane provenance: which lanes ranked this chunk and at what 1-based rank.
/// `lane_ranks` follows the fixed lane order passed to [`rrf_fuse_detailed`] (the caller orders it
/// exact → vector → keyword), listing only the lanes that actually ranked this chunk. It is NOT
/// sorted by per-lane contribution, so `lane_ranks[0]` is the highest-precedence *present* lane
/// (exact when it fired), not necessarily the largest score term.
pub struct FusedHit {
    /// The `<hash>:<ordinal>` join key.
    pub chunk_id: String,
    /// Summed RRF score across the lanes that ranked this chunk.
    pub score: f32,
    /// `(lane_name, 1-based rank)` for each lane that ranked this chunk, in lane order.
    pub lane_ranks: Vec<(&'static str, u32)>,
}

/// Fuse ranked lanes via RRF, retaining per-lane provenance. Returns [`FusedHit`]s sorted by score
/// descending, with a stable ascending-`chunk_id` tie-break so the order is deterministic across
/// runs. Empty lanes (and an empty `lanes` slice) contribute nothing. Each hit's `lane_ranks`
/// records every lane that ranked the chunk and at what 1-based rank, in the lane order passed in.
pub fn rrf_fuse_detailed(lanes: &[FusionLane<'_>], k: f32) -> Vec<FusedHit> {
    #[derive(Default)]
    struct Acc {
        score: f32,
        lane_ranks: Vec<(&'static str, u32)>,
    }
    let mut acc: AHashMap<&str, Acc> = AHashMap::new();
    for lane in lanes {
        let mut seen_in_lane: AHashSet<&str> = AHashSet::new();
        for (rank0, chunk_id) in lane.chunk_ids.iter().enumerate() {
            if !seen_in_lane.insert(chunk_id.as_str()) {
                continue;
            }
            let rank = (rank0 + 1) as u32;
            let entry = acc.entry(chunk_id.as_str()).or_default();
            entry.score += lane.weight / (k + rank as f32);
            entry.lane_ranks.push((lane.name, rank));
        }
    }
    let mut fused: Vec<FusedHit> = acc
        .into_iter()
        .map(|(id, a)| FusedHit {
            chunk_id: id.to_string(),
            score: a.score,
            lane_ranks: a.lane_ranks,
        })
        .collect();
    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    fused
}

/// Fuse ranked lanes via RRF. Returns `(chunk_id, fused_score)` sorted by score descending, with a
/// stable ascending-`chunk_id` tie-break so the order is deterministic across runs. A thin
/// projection of [`rrf_fuse_detailed`] for callers that don't need per-lane provenance.
pub fn rrf_fuse(lanes: &[FusionLane<'_>], k: f32) -> Vec<(String, f32)> {
    rrf_fuse_detailed(lanes, k)
        .into_iter()
        .map(|h| (h.chunk_id, h.score))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn agreeing_lanes_rank_the_shared_top_first() {
        let a = ids(&["h:1", "h:2", "h:3"]);
        let b = ids(&["h:1", "h:3", "h:4"]);
        let fused = rrf_fuse(
            &[
                FusionLane::new(LANE_KEYWORD, &a, 1.0),
                FusionLane::new(LANE_VECTOR, &b, 1.0),
            ],
            DEFAULT_RRF_K,
        );
        assert_eq!(fused[0].0, "h:1");
        assert!(fused[0].1 > fused[1].1);
        let uniq: std::collections::HashSet<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(uniq.len(), 4);
    }

    #[test]
    fn weight_boosts_a_lane_that_ranks_a_chunk() {
        let exact = ids(&["h:def"]);
        let keyword = ids(&["h:other", "h:def"]);
        let fused = rrf_fuse(
            &[
                FusionLane::new(LANE_EXACT, &exact, WEIGHT_EXACT),
                FusionLane::new(LANE_KEYWORD, &keyword, WEIGHT_KEYWORD),
            ],
            DEFAULT_RRF_K,
        );
        assert_eq!(fused[0].0, "h:def", "exact-lane rank-1 with 2x weight must win");
    }

    #[test]
    fn duplicate_within_lane_counts_once() {
        let dupe = ids(&["h:1", "h:1", "h:1"]);
        let single = ids(&["h:1"]);
        let a = rrf_fuse(&[FusionLane::new(LANE_KEYWORD, &dupe, 1.0)], DEFAULT_RRF_K);
        let b = rrf_fuse(&[FusionLane::new(LANE_KEYWORD, &single, 1.0)], DEFAULT_RRF_K);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].1, b[0].1, "repeats of a chunk within a lane must not stack");
    }

    #[test]
    fn empty_lanes_produce_empty_output() {
        let empty: Vec<String> = Vec::new();
        assert!(rrf_fuse(&[FusionLane::new(LANE_KEYWORD, &empty, 1.0)], DEFAULT_RRF_K).is_empty());
        assert!(rrf_fuse(&[], DEFAULT_RRF_K).is_empty());
    }

    /// An empty lane must leave the fused order **untouched**, not merely produce an empty result.
    ///
    /// The existing test above covers only the all-empty case. The property that matters in
    /// production is the mixed one: `hybrid` runs its lanes best-effort and a lane that failed or
    /// matched nothing must not perturb the survivors. If an empty lane contributed a zero-weight
    /// score entry — or worse, a rank — every other lane's ordering would shift and a degraded
    /// lane would silently rewrite results.
    #[test]
    fn an_empty_lane_leaves_the_fused_order_of_the_others_unchanged() {
        let empty: Vec<String> = Vec::new();
        let exact = ids(&["h:aaa", "h:bbb"]);
        let vector = ids(&["h:ccc", "h:aaa"]);

        let without = rrf_fuse(
            &[
                FusionLane::new(LANE_EXACT, &exact, 2.0),
                FusionLane::new(LANE_VECTOR, &vector, 1.0),
            ],
            DEFAULT_RRF_K,
        );
        let with = rrf_fuse(
            &[
                FusionLane::new(LANE_EXACT, &exact, 2.0),
                // Inert: a failed or all-filtered lane arrives here with no ids.
                FusionLane::new(LANE_VECTOR, &empty, 1.0),
                FusionLane::new(LANE_KEYWORD, &vector, 1.0),
            ],
            DEFAULT_RRF_K,
        );

        assert_eq!(without.len(), 3, "three distinct chunks across the two live lanes");
        assert_eq!(
            with.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            without.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            "adding an empty lane must not reorder or rescore anything"
        );
    }

    /// The documents tier's spec §5 weights are reachable without touching the code tier's.
    ///
    /// Two sets of weights are called "the documented weights" and they disagree, so both are pinned
    /// here. `default()` must stay the code tier's values: swapping in §5's would change the tier
    /// that ships today, and `search_code_hybrid_ranks_exact_symbol_first` would catch it — but only
    /// once someone ran it, which is not a reason to leave the distinction implicit.
    #[test]
    fn code_weights_are_the_default_and_document_weights_are_separate() {
        let code = FusionWeights::default();
        assert_eq!(
            (code.exact, code.keyword, code.vector),
            (2.0, 1.0, 1.0),
            "the code tier's shipped weights: exact highest, keyword and vector equal"
        );

        let docs = FusionWeights::document_lanes();
        assert_eq!(
            (docs.exact, docs.keyword, docs.facet, docs.vector),
            (3.0, 2.0, 1.0, 1.0)
        );

        assert_ne!(
            FusionWeights::default(),
            FusionWeights::document_lanes(),
            "the two sets must not collapse into one"
        );
    }

    /// A four-lane fusion at spec §5 weights, asserted against an explicitly constructed struct so
    /// the test cannot pass by reading `default()`.
    #[test]
    fn four_lane_fusion_at_document_weights() {
        let w = FusionWeights::document_lanes();
        let exact = ids(&["h:one"]);
        let keyword = ids(&["h:one", "h:two"]);
        let facet = ids(&["h:one", "h:two", "h:three"]);
        let vector = ids(&["h:four"]);

        let fused = rrf_fuse_detailed(
            &[
                FusionLane::new(LANE_EXACT, &exact, w.exact),
                FusionLane::new(LANE_KEYWORD, &keyword, w.keyword),
                FusionLane::new(LANE_FACET, &facet, w.facet),
                FusionLane::new(LANE_VECTOR, &vector, w.vector),
            ],
            DEFAULT_RRF_K,
        );

        assert_eq!(fused.len(), 4, "all four lanes contribute");
        assert_eq!(fused[0].chunk_id, "h:one", "the chunk every lane ranked first wins");
        // Rank-only: a lane that contributes one chunk scores the same as any other such lane,
        // whatever its weight. h:four is the vector lane's only hit, at rank 1.
        let four = fused.iter().find(|f| f.chunk_id == "h:four").expect("h:four present");
        let score = four.score;
        assert!(score > 0.0, "the vector lane's rank-1 hit scores above zero");
        assert!(
            fused.iter().all(|f| f.score >= score || f.chunk_id != "h:one"),
            "h:one accumulates four lanes and must not rank below a single-lane hit"
        );
    }

    /// An unknown lane name weighs zero rather than panicking, so a tier naming a lane this build
    /// does not know loses that lane instead of the whole query.
    #[test]
    fn an_unknown_lane_name_weighs_zero() {
        let w = FusionWeights::code_lanes();
        assert_eq!(w.for_lane(LANE_EXACT), 2.0);
        assert_eq!(w.for_lane(LANE_VECTOR), 1.0);
        assert_eq!(w.for_lane(LANE_KEYWORD), 1.0);
        assert_eq!(w.for_lane(LANE_FACET), 0.0, "code tier has no facet lane");
        assert_eq!(w.for_lane("phrase"), 0.0, "a lane this build does not know is inert");
    }

    #[test]
    fn equal_scores_break_ties_by_chunk_id_ascending() {
        let a = ids(&["h:zzz"]);
        let b = ids(&["h:aaa"]);
        let fused = rrf_fuse(
            &[
                FusionLane::new(LANE_KEYWORD, &a, 1.0),
                FusionLane::new(LANE_VECTOR, &b, 1.0),
            ],
            DEFAULT_RRF_K,
        );
        assert_eq!(fused[0].0, "h:aaa");
        assert_eq!(fused[1].0, "h:zzz");
    }

    #[test]
    fn detailed_fusion_records_per_lane_ranks_in_lane_order() {
        let exact = ids(&["h:9", "h:1"]);
        let keyword = ids(&["h:1"]);
        let fused = rrf_fuse_detailed(
            &[
                FusionLane::new(LANE_EXACT, &exact, WEIGHT_EXACT),
                FusionLane::new(LANE_KEYWORD, &keyword, WEIGHT_KEYWORD),
            ],
            DEFAULT_RRF_K,
        );
        let h1 = fused.iter().find(|h| h.chunk_id == "h:1").expect("h:1 present");
        assert_eq!(h1.lane_ranks, vec![(LANE_EXACT, 2), (LANE_KEYWORD, 1)]);
        let h9 = fused.iter().find(|h| h.chunk_id == "h:9").expect("h:9 present");
        assert_eq!(
            h9.lane_ranks,
            vec![(LANE_EXACT, 1)],
            "single-lane hit records only that lane"
        );
    }
}
