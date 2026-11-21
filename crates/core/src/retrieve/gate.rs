//! The relevance gate: what a candidate must show before search returns it. A vector channel
//! always lists its nearest neighbours, so without a gate an unrelated query still gets cards.

/// Share of the query's lexemes the memory contains.
pub const COVERAGE_MIN: f32 = 0.5;
/// Cosine that keeps a hit with no word in common (a paraphrase).
pub const COSINE_STRONG: f32 = 0.40;
/// Cosine that keeps a hit with some word in common.
pub const COSINE_WEAK: f32 = 0.25;

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
        || g.coverage >= COVERAGE_MIN
        || g.cosine
            .is_some_and(|c| c >= COSINE_STRONG || (g.coverage > 0.0 && c >= COSINE_WEAK))
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
        assert!(keep(&input(false, 0.2, Some(COSINE_WEAK))));
        assert!(!keep(&input(false, 0.0, Some(COSINE_WEAK))));
    }

    #[test]
    fn gate_bounds_are_exact_edges() {
        let just_under = |bound: f32| bound - 0.001;
        assert!(!keep(&input(false, 0.0, Some(just_under(COSINE_STRONG)))));
        assert!(!keep(&input(false, 0.2, Some(just_under(COSINE_WEAK)))));
        assert!(!keep(&input(false, just_under(COVERAGE_MIN), None)));
        // A single shared word in a long query keeps nothing on its own, whatever the corpus size.
        assert!(!keep(&input(false, 0.2, None)));
    }

    #[test]
    fn gate_drops_single_shared_word_without_meaning() {
        assert!(!keep(&input(false, 0.33, Some(0.10))));
        assert!(!keep(&input(false, 0.33, None)));
        assert!(!keep(&input(false, 0.0, None)));
    }
}
