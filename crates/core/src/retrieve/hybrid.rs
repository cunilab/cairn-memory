use crate::memory::Kind;
use std::collections::{HashMap, HashSet};

/// Reciprocal-rank-fusion constant. Same algorithm as 0.1.5; only the channels change in step 5.
pub const RRF_K: f32 = 60.0;
pub const CANDIDATES_PER_CHANNEL: usize = 512;
const RULE_DECISION_WEIGHT: f32 = 3.0;
const NOTE_WEIGHT: f32 = 2.0;
const HANDOFF_WEIGHT: f32 = 1.0;

pub fn kind_weight(kind: Kind) -> f32 {
    match kind {
        Kind::Rule | Kind::Decision => RULE_DECISION_WEIGHT,
        Kind::Handoff => HANDOFF_WEIGHT,
        _ => NOTE_WEIGHT,
    }
}

/// Fuses ranked channels (lexical, entity, semantic). Each channel is `(id, score)`; non-finite or
/// non-positive scores are dropped, repeats keep the best, ties break by id.
pub fn fuse(channels: Vec<Vec<(String, f32)>>) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for mut channel in channels {
        channel.retain(|(_, score)| score.is_finite() && *score > 0.0);
        sort(&mut channel);
        let mut seen = HashSet::new();
        channel.retain(|(id, _)| seen.insert(id.clone()));
        channel.truncate(CANDIDATES_PER_CHANNEL);
        for (rank, (id, _)) in channel.into_iter().enumerate() {
            *scores.entry(id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    let mut fused: Vec<_> = scores.into_iter().collect();
    sort(&mut fused);
    fused
}

fn sort(channel: &mut [(String, f32)]) {
    channel.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(items: &[(&str, f32)]) -> Vec<(String, f32)> {
        items.iter().map(|(id, s)| ((*id).to_owned(), *s)).collect()
    }

    #[test]
    fn agreement_across_channels_wins() {
        let fused = fuse(vec![
            ch(&[("a", 9.0), ("b", 5.0)]),
            ch(&[("b", 0.9), ("c", 0.8)]),
        ]);
        assert_eq!(fused[0].0, "b");
        assert_eq!(fused.len(), 3);
    }

    #[test]
    fn bad_scores_and_repeats_are_dropped() {
        let fused = fuse(vec![ch(&[
            ("a", f32::NAN),
            ("b", 0.0),
            ("c", 1.0),
            ("c", 0.5),
        ])]);
        assert_eq!(fused, vec![("c".to_owned(), 1.0 / (RRF_K + 1.0))]);
    }

    #[test]
    fn rules_outweigh_notes() {
        assert!(kind_weight(Kind::Rule) > kind_weight(Kind::Fact));
        assert!(kind_weight(Kind::Fact) > kind_weight(Kind::Handoff));
    }
}
