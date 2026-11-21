use std::collections::{HashMap, HashSet};

/// Reciprocal-rank-fusion constant. Same algorithm as 0.1.5; only the channels change in step 5.
pub const RRF_K: f32 = 60.0;
pub const CANDIDATES_PER_CHANNEL: usize = 64;
/// Fused hits kept before the request filter and the relevance gate trim them.
pub const POOL: usize = 128;
/// Fuses ranked channels (lexical, entity, semantic, neighbours). Each channel is `(id, score)`;
/// non-finite or non-positive scores are dropped and repeats keep the best. Ties keep the order
/// the caller gave: a channel's own order for equal scores, the first channel that listed an id for
/// equal fused scores. Ids are random, so the caller breaks ties in its query (newest first), not
/// here.
pub fn fuse(channels: Vec<Vec<(String, f32)>>) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for mut channel in channels {
        channel.retain(|(_, score)| score.is_finite() && *score > 0.0);
        channel.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut seen = HashSet::new();
        channel.retain(|(id, _)| seen.insert(id.clone()));
        channel.truncate(CANDIDATES_PER_CHANNEL);
        for (rank, (id, _)) in channel.into_iter().enumerate() {
            if !scores.contains_key(&id) {
                order.push(id.clone());
            }
            *scores.entry(id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    let mut fused: Vec<_> = order
        .into_iter()
        .map(|id| {
            let score = scores[&id];
            (id, score)
        })
        .collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1));
    fused
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
    fn ties_keep_the_callers_order() {
        let fused = fuse(vec![ch(&[("z", 1.0), ("a", 1.0)]), ch(&[("m", 1.0)])]);
        let ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["z", "m", "a"],
            "equal scores keep channel order, not id order"
        );
    }
}
