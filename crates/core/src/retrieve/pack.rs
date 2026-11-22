//! The shapes search returns, and the diversity cap, token budget and limit that keep a response small.

use crate::memory::{EntityKind, Kind, Memory, Status};
use serde::Serialize;
use std::collections::HashMap;

/// A memory plus its search score.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub memory: Memory,
    pub score: f32,
}

/// What an agent sees of a hit; `memory_get` fetches the full record.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Card {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    /// Only when the memory is not `active` (a stale memory still ranks, lower).
    #[serde(default, skip_serializing_if = "is_active")]
    pub status: Status,
    /// Rounded to 3 decimals, relative to the top card (1.0).
    pub score: f32,
}

/// `more` counts hits cut by the limit or the budget; a caller raises either to see them.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cards {
    pub results: Vec<Card>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_active(status: &Status) -> bool {
    *status == Status::Active
}

/// Default response budget, in tokens (estimated as bytes / 4, as the bench does).
pub const BUDGET_DEFAULT: usize = 1200;
const BUDGET_MIN: usize = 200;
const BUDGET_MAX: usize = 4000;
/// Cards one file or symbol may fill before other hits go first.
const PER_ENTITY: u8 = 2;

fn file_and_symbol_names(memory: &Memory) -> Vec<&str> {
    memory
        .entities
        .iter()
        .filter(|e| matches!(e.kind, EntityKind::File | EntityKind::Symbol))
        .map(|e| e.name.as_str())
        .collect()
}

/// Best first: score descending, then newest, then id.
pub fn sort_best_first(hits: &mut [Hit]) {
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.memory.created_us.cmp(&a.memory.created_us))
            .then_with(|| a.memory.id.cmp(&b.memory.id))
    });
}

struct Packer {
    /// Each card with its index in `hits`, so the response can be put back in score order.
    out: Vec<(usize, Card)>,
    used: usize,
    budget: usize,
    limit: usize,
    top: f32,
}

impl Packer {
    /// Adds the card unless the limit or the byte budget is reached; the first card always fits.
    fn add(&mut self, index: usize, hit: &Hit) -> bool {
        let card = Card {
            id: hit.memory.id.clone(),
            kind: hit.memory.kind,
            title: hit.memory.title.clone(),
            summary: hit.memory.summary.clone(),
            status: hit.memory.status,
            score: if self.top > 0.0 {
                (hit.score / self.top * 1000.0).round() / 1000.0
            } else {
                1.0
            },
        };
        let cost = serde_json::to_string(&card).map_or(0, |json| json.len()) + 1;
        if self.out.len() == self.limit || (self.used + cost > self.budget && !self.out.is_empty())
        {
            return false;
        }
        self.used += cost;
        self.out.push((index, card));
        true
    }
}

/// `hits` arrive best first. Two cards per file or symbol entity go first, then the rest fill what
/// is left of the budget (`budget_tokens` clamped to 200..=4000) and `limit`. `more` counts the
/// hits not returned.
pub fn pack(hits: &[Hit], limit: usize, budget_tokens: usize) -> Cards {
    let mut packer = Packer {
        out: Vec::new(),
        used: 0,
        budget: budget_tokens.clamp(BUDGET_MIN, BUDGET_MAX) * 4,
        limit,
        top: hits.first().map_or(0.0, |h| h.score),
    };
    let mut per_entity: HashMap<&str, u8> = HashMap::new();
    let mut deferred = Vec::new();
    for (index, hit) in hits.iter().enumerate() {
        if packer.out.len() == limit {
            break;
        }
        let keys = file_and_symbol_names(&hit.memory);
        if keys
            .iter()
            .any(|k| per_entity.get(k).copied().unwrap_or(0) >= PER_ENTITY)
        {
            deferred.push((index, hit));
            continue;
        }
        // A card that does not fit leaves room for a smaller one further down.
        if packer.add(index, hit) {
            for key in keys {
                *per_entity.entry(key).or_default() += 1;
            }
        }
    }
    for (index, hit) in deferred {
        if packer.out.len() == limit {
            break;
        }
        packer.add(index, hit);
    }
    packer.out.sort_by_key(|(index, _)| *index);
    let results: Vec<Card> = packer.out.into_iter().map(|(_, card)| card).collect();
    // `hits` is already capped at the pool, so `more` never counts beyond it.
    Cards {
        more: hits.len() - results.len(),
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Entity, Scope};
    use crate::testing::memory;

    fn hit(id: &str, score: f32) -> Hit {
        Hit {
            memory: memory(id, Kind::Fact, Scope::Personal),
            score,
        }
    }

    fn on_file(id: &str, score: f32, file: &str) -> Hit {
        let mut h = hit(id, score);
        h.memory.entities.push(Entity {
            kind: EntityKind::File,
            name: file.into(),
        });
        h
    }

    fn ids(cards: &Cards) -> Vec<&str> {
        cards.results.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn cards_cut_normalise_and_count_the_rest() {
        let c = pack(&[hit("a", 0.8), hit("b", 0.2), hit("c", 0.1)], 2, 1200);
        assert_eq!((c.results.len(), c.more), (2, 1));
        assert_eq!((c.results[0].score, c.results[1].score), (1.0, 0.25));
        let json = serde_json::to_value(pack(&[hit("a", 0.3)], 5, 1200)).expect("json");
        assert!(json.get("more").is_none());
        let keys: Vec<_> = json["results"][0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "kind", "score", "summary", "title"]);
        let mut stale = hit("s", 0.3);
        stale.memory.status = Status::Stale;
        let json = serde_json::to_value(pack(&[stale], 5, 1200)).expect("json");
        assert_eq!(json["results"][0]["status"], "stale");
    }

    #[test]
    fn pack_caps_two_per_entity_then_fills() {
        let hits = [
            on_file("a", 1.0, "src/x.rs"),
            on_file("b", 0.9, "src/x.rs"),
            on_file("c", 0.8, "src/x.rs"),
            on_file("d", 0.7, "src/y.rs"),
        ];
        assert_eq!(ids(&pack(&hits, 3, 1200)), ["a", "b", "d"]);
        // With room left the deferred hit comes too, and the response stays in score order.
        assert_eq!(ids(&pack(&hits, 4, 1200)), ["a", "b", "c", "d"]);
    }

    #[test]
    fn pack_tries_a_smaller_card_after_one_that_does_not_fit() {
        let mut big = hit("big", 0.9);
        big.memory.summary = "s".repeat(240);
        let mut hits = vec![hit("a", 1.0)];
        hits.extend((0..4).map(|i| {
            let mut h = big.clone();
            h.memory.id = format!("big{i}");
            h
        }));
        hits.push(hit("small", 0.1));
        let c = pack(&hits, 10, 200);
        assert_eq!(ids(&c).first(), Some(&"a"));
        assert_eq!(ids(&c).last(), Some(&"small"), "{:?}", ids(&c));
    }

    #[test]
    fn best_first_breaks_ties_on_newest_then_id() {
        let at = |id: &str, score: f32, us: i64| {
            let mut h = hit(id, score);
            h.memory.created_us = us;
            h
        };
        let mut hits = vec![
            at("b", 0.5, 1),
            at("c", 0.5, 2),
            at("a", 0.5, 2),
            at("z", 0.9, 0),
        ];
        sort_best_first(&mut hits);
        let order: Vec<&str> = hits.iter().map(|h| h.memory.id.as_str()).collect();
        assert_eq!(order, ["z", "a", "c", "b"]);
    }

    #[test]
    fn pack_respects_budget_and_limit() {
        let mut hits: Vec<Hit> = (0..10).map(|i| hit(&format!("m{i}"), 1.0)).collect();
        for h in &mut hits {
            h.memory.summary = "s".repeat(240);
        }
        let c = pack(&hits, 10, 200);
        assert!(
            c.results.len() < 10 && c.results.len() >= 2,
            "{}",
            c.results.len()
        );
        let bytes: usize = c
            .results
            .iter()
            .map(|c| serde_json::to_string(c).unwrap().len() + 1)
            .sum();
        assert!(bytes <= 800, "{bytes}");
        assert_eq!(c.more, 10 - c.results.len());
        assert_eq!(pack(&hits, 3, 4000).results.len(), 3);
    }

    #[test]
    fn pack_keeps_one_card_over_budget() {
        let mut big = hit("a", 1.0);
        big.memory.summary = "s".repeat(2000);
        let c = pack(&[big, hit("b", 0.5)], 5, 200);
        assert_eq!(ids(&c), ["a"]);
        assert_eq!(c.more, 1);
    }
}
