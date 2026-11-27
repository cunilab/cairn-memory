//! The memory model: kinds, scopes, per-kind fields, validation, identity and kind policy.

pub mod coaching;
pub mod fields;
pub mod identity;
pub mod kind;
pub mod literal;
pub mod model;
mod trust;
pub mod validate;

pub use fields::Fields;
pub use literal::{
    LiteralMatch, MatchSegment, TextPart, best_match, contains_literal, memory_parts, title_equals,
};
pub use model::{
    Context, Entity, EntityKind, Kind, Link, Memory, Record, Relation, SaveInput, Scope, Status,
    TrustLevel, Usage,
};

pub use trust::{PROVEN_HELPFUL, PROVEN_SAVES};
