//! Cairn core: pure memory types and logic. No IO, no database, no async runtime.
//!
//! One module per area: `memory` is the model and depends on nothing; `capture`, `process`,
//! `retrieve`, `project` and `account` depend only on `memory` and each other.

pub mod account;
pub mod capture;
pub mod error;
pub mod fixes;
pub mod id;
pub mod instructions;
pub mod memory;
pub mod process;
pub mod project;
pub mod retrieve;
pub mod skill;
#[cfg(test)]
mod testing;
