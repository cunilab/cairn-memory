//! Derivation and background work, all pure: the save pipeline, entities, embedding text,
//! consolidation bands, distillation and job policy. Depends on `memory` and `capture`.

pub mod consolidate;
pub mod distill;
pub mod embed;
pub mod entities;
pub mod jobs;
pub mod prepare;

pub use prepare::{NewMemory, Origin, prepare_save};
