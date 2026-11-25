//! The memory model: kinds, scopes, per-kind fields, validation, identity and kind policy.

pub mod coaching;
pub mod fields;
pub mod identity;
pub mod kind;
pub mod model;
mod trust;
pub mod validate;

pub use fields::Fields;
pub use model::{
    Context, Entity, EntityKind, Kind, Link, Memory, Record, Relation, SaveInput, Scope, Status,
    TrustLevel, Usage,
};
