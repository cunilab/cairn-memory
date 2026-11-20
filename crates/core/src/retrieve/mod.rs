//! Retrieval, all pure: request filter and relevance floor, rank fusion and the feedback
//! multiplier, the result pack, the session brief and the graph. Depends on `memory`,
//! `capture` and `process` (for the handoff id).

pub mod brief;
pub mod filter;
pub mod graph;
pub mod hybrid;
pub mod pack;
pub mod rank;
