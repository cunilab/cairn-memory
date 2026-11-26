//! Capture: raw observations, sessions and redaction. Depends on `memory` and `error`.

mod chat;
pub mod observation;
pub mod redact;
pub mod session;

pub use chat::{ChatContext, ChatHash, ChatKind, ChatPlatform};
pub use observation::{
    CaptureKind, CaptureResult, Observation, ObservationInput, WorkContext, normalize_observation,
};
pub use redact::{REDACTED, redact_sensitive_content, redact_work_context};
pub use session::Session;
