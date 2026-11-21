//! The shapes search returns, and the limit that keeps a response small.

use crate::memory::{Kind, Memory};
use serde::Serialize;

/// A memory plus its search score.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub memory: Memory,
    pub score: f32,
}

/// What an agent sees of a hit; `memory_get` fetches the full record.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Card {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    /// Rounded to 3 decimals, relative to the top card (1.0).
    pub score: f32,
}

/// `more` counts hits cut by the limit; a caller raises the limit to see them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cards {
    pub results: Vec<Card>,
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// `hits` arrive best first.
pub fn cards(mut hits: Vec<Hit>, limit: usize) -> Cards {
    let more = hits.len().saturating_sub(limit);
    hits.truncate(limit);
    let top = hits.first().map_or(0.0, |h| h.score);
    let results = hits
        .into_iter()
        .map(|h| Card {
            id: h.memory.id,
            kind: h.memory.kind,
            title: h.memory.title,
            summary: h.memory.summary,
            score: if top > 0.0 {
                (h.score / top * 1000.0).round() / 1000.0
            } else {
                1.0
            },
        })
        .collect();
    Cards { results, more }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Scope;
    use crate::testing::memory;

    #[test]
    fn cards_cut_normalise_and_count_the_rest() {
        let hit = |id: &str, score: f32| Hit {
            memory: memory(id, Kind::Fact, Scope::Personal),
            score,
        };
        let c = cards(vec![hit("a", 0.8), hit("b", 0.2), hit("c", 0.1)], 2);
        assert_eq!((c.results.len(), c.more), (2, 1));
        assert_eq!((c.results[0].score, c.results[1].score), (1.0, 0.25));
        let json = serde_json::to_value(cards(vec![hit("a", 0.3)], 5)).expect("json");
        assert!(json.get("more").is_none());
        let keys: Vec<_> = json["results"][0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "kind", "score", "summary", "title"]);
    }
}
