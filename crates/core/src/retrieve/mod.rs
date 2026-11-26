//! Retrieval, all pure: request filter, rank fusion, the relevance gate and score adjustments,
//! the result pack, the session brief and the graph. Depends on `memory`, `capture` and `process`
//! (for the handoff id).

pub mod brief;
pub mod command;
pub mod filter;
pub mod gate;
pub mod graph;
pub mod hybrid;
pub mod pack;
pub mod prompt;

pub mod rank;
