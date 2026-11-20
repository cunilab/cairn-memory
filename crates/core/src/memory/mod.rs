//! The memory model: kinds, scopes, per-kind fields, validation, identity and kind policy.
//! Depends on nothing else in this crate except `error`.

pub mod fields;
pub mod identity;
pub mod kind;
pub mod model;
pub mod validate;

pub use fields::Fields;
pub use model::{
    Context, Entity, EntityKind, Kind, Link, Memory, Relation, SaveInput, Scope, Status, Usage,
};
