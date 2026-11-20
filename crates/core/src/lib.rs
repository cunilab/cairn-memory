//! Cairn core: pure memory types and logic. No IO, no database, no async runtime.
//!
//! The pre-0.1.6 flat modules live in `legacy` and are re-exported unchanged until step 2c
//! switches the server to `cairn-store` and deletes them. New code is under the area modules
//! below and is reached by path (`cairn_core::memory::Memory`), so no name clashes with legacy.

mod legacy;
pub use legacy::*;

pub mod account;
pub mod capture;
pub mod error;
pub mod id;
pub mod memory;
pub mod process;
pub mod project;
pub mod retrieve;
#[cfg(test)]
mod testing;
