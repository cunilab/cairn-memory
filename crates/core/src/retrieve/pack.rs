//! The shapes search returns, and the limit that keeps a response small.

use crate::memory::Memory;
use serde::Serialize;

/// A memory plus its search score. `usage` never serializes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    #[serde(flatten)]
    pub memory: Memory,
    pub score: f32,
}

/// `more` counts hits cut by the limit; a caller raises the limit to see them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pack {
    pub query: String,
    pub results: Vec<Hit>,
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

pub fn pack(query: impl Into<String>, mut hits: Vec<Hit>, limit: usize) -> Pack {
    let more = hits.len().saturating_sub(limit);
    hits.truncate(limit);
    Pack {
        query: query.into(),
        results: hits,
        more,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Kind, Scope};
    use crate::testing::memory;

    #[test]
    fn limit_counts_the_rest() {
        let hit = |id: &str| Hit {
            memory: memory(id, Kind::Fact, Scope::Personal),
            score: 1.0,
        };
        let p = pack("q", vec![hit("a"), hit("b"), hit("c")], 2);
        assert_eq!((p.results.len(), p.more), (2, 1));
        let json = serde_json::to_value(pack("q", vec![hit("a")], 5)).expect("json");
        assert!(json.get("more").is_none());
        assert_eq!(json["results"][0]["id"], "a");
        assert!(json["results"][0].get("usage").is_none());
    }
}
