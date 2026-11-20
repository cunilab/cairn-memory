//! Projects and the visibility predicate. Depends on `memory`.

pub mod access;
pub mod model;

pub use access::{Viewer, is_visible};
pub use model::{Project, default_project_name, validate_project_text};
