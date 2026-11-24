//! The relevance gate and the absolute relevance score. A vector channel always lists its nearest
//! neighbours, so without a gate an unrelated query still gets cards; and a rank-only score
//! (reciprocal-rank fusion) says nothing about how good a match is: on a store of three notes it
//! is the same for a perfect and a poor match, so the kind and freshness adjustments, not the
//! text, decided the order.

/// Share of the query's lexemes the memory contains.
pub const COVERAGE_MIN: f32 = 0.5;
/// Cosine that keeps a hit with no word in common (a paraphrase). Measured on bench v2 (see
/// `bench/README.md`): 0.43 leaks no weak query.
pub const COSINE_STRONG: f32 = 0.43;
/// Cosine a words-only hit (coverage at least `COVERAGE_MIN`) needs when the memory has an
/// embedding. A word the query shares with every note of a store (the project's own name) is not
/// evidence on its own. On bench v2 every kept gold with coverage >= 0.5 has cosine >= 0.38.
pub const COSINE_WITH_WORDS: f32 = 0.35;
/// A query that names a file or symbol the memory cites is a strong match by itself.
pub const ENTITY_RELEVANCE: f32 = 0.7;
/// Cosine at and below this adds nothing to the relevance; `1.0` adds everything.
const COSINE_FLOOR: f32 = 0.25;

/// What the candidate load measured for one hit.
#[derive(Clone, Copy, Debug)]
pub struct GateInput {
    pub entity_hit: bool,
    pub coverage: f32,
    /// `None` without a query vector or a stored embedding.
    pub cosine: Option<f32>,
}

pub fn keep(g: &GateInput) -> bool {
    g.entity_hit
        || g.cosine.is_some_and(|c| c >= COSINE_STRONG)
        || (g.coverage >= COVERAGE_MIN && g.cosine.is_none_or(|c| c >= COSINE_WITH_WORDS))
}

/// How well the memory matches the query, 0.0..=1.0, from the same measurements as the gate and
/// independent of every other candidate: the mean of the words it shares (coverage) and the
/// meaning it shares (cosine, 0 at 0.25, 1 at 1.0). A memory with no embedding yet (just saved)
/// is judged on its words alone, so it is not ranked below the rest for being new. An entity hit
/// is at least `ENTITY_RELEVANCE`.
pub fn relevance(g: &GateInput) -> f32 {
    let words = g.coverage.clamp(0.0, 1.0);
    let base = match g.cosine {
        Some(c) => (words + ((c - COSINE_FLOOR) / (1.0 - COSINE_FLOOR)).clamp(0.0, 1.0)) / 2.0,
        None => words,
    };
    if g.entity_hit {
        base.max(ENTITY_RELEVANCE)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(entity_hit: bool, coverage: f32, cosine: Option<f32>) -> GateInput {
        GateInput {
            entity_hit,
            coverage,
            cosine,
        }
    }

    #[test]
    fn gate_keeps_entity_coverage_or_cosine() {
        assert!(keep(&input(true, 0.0, None)));
        assert!(keep(&input(false, 0.5, None)));
        assert!(keep(&input(false, 0.0, Some(COSINE_STRONG))));
        // Some shared words do not lower the cosine bar.
        assert!(!keep(&input(false, 0.2, Some(0.30))));
    }

    #[test]
    fn gate_bounds_are_exact_edges() {
        let just_under = |bound: f32| bound - 0.001;
        assert!(!keep(&input(false, 0.0, Some(just_under(COSINE_STRONG)))));
        assert!(!keep(&input(false, just_under(COVERAGE_MIN), None)));
        // A single shared word in a long query keeps nothing on its own, whatever the corpus size.
        assert!(!keep(&input(false, 0.2, None)));
    }

    #[test]
    fn gate_asks_words_only_hits_for_some_meaning_when_there_is_an_embedding() {
        // "cairn" is in every note of a Cairn store: half the query, but no evidence by itself.
        assert!(!keep(&input(false, 0.5, Some(0.30))));
        assert!(keep(&input(false, 0.5, Some(COSINE_WITH_WORDS))));
        // Without an embedding (a note saved a moment ago) the words decide.
        assert!(keep(&input(false, 0.5, None)));
        // A paraphrase and an entity hit need no words.
        assert!(keep(&input(false, 0.0, Some(COSINE_STRONG))));
        assert!(keep(&input(true, 0.0, Some(0.1))));
    }

    #[test]
    fn relevance_is_absolute_and_ignores_the_other_candidates() {
        let r = |coverage, cosine| relevance(&input(false, coverage, cosine));
        // A full match with close meaning beats a half match, whoever else is in the store.
        assert!(r(1.0, Some(0.6)) > r(0.5, Some(0.4)));
        assert!((r(1.0, Some(1.0)) - 1.0).abs() < 1e-6);
        assert_eq!(r(0.0, Some(0.2)), 0.0);
        // No embedding yet: the words alone, not a penalty for being new.
        assert_eq!(r(1.0, None), 1.0);
        assert_eq!(r(0.5, None), 0.5);
        // An entity hit has a floor.
        assert_eq!(relevance(&input(true, 0.0, Some(0.2))), ENTITY_RELEVANCE);
        assert_eq!(relevance(&input(true, 1.0, None)), 1.0);
    }

    #[test]
    fn gate_drops_single_shared_word_without_meaning() {
        assert!(!keep(&input(false, 0.33, Some(0.10))));
        assert!(!keep(&input(false, 0.33, None)));
        assert!(!keep(&input(false, 0.0, None)));
    }
}
