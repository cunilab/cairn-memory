//! Layer 1/2 personal memory and continuity primitives.
//!
//! This crate intentionally implements a small, local vertical slice:
//! capture -> redact -> persist -> summarize -> scoped recall -> forget. The
//! file store is a prototype persistence adapter for the local continuity
//! layers; the server-authoritative PostgreSQL adapter remains later.

mod accounts;
mod brief;
mod capture;
mod devices;
mod distillation;
mod embeddings;
mod hybrid;
mod lifecycle;
mod memories;
mod ownership;
mod privacy;
mod projects;
mod recall;
mod retention;
mod sessions;
mod sharing;
mod store;
mod sync;
#[cfg(test)]
mod test_support;
mod transfer;
mod types;

pub use accounts::{
    ADMIN_ONLY_ERROR, AUTH_SESSION_SECONDS, Account, AccountStatus, AuditEvent, AuthSessionResult,
    WEB_ONLY_ERROR, hash_password, hash_token, normalize_email, random_token, validate_password,
    verify_password,
};
pub use brief::{SESSION_BRIEF_MAX_BYTES, session_brief};
pub use capture::{CaptureResult, Observation, ObservationInput, normalize_observation};
pub use devices::{DEVICE_TOKEN_PREFIX, Device, DeviceRegistration, DeviceStatus};
pub use distillation::distill_session_notes;
pub use lifecycle::{
    Lifecycle, LifecycleInput, LifecycleSummary, MemoryKind, MemoryScope, Retention, Sensitivity,
    sensitivity_including_context,
};
pub use memories::{
    Memory, MemoryFields, MemoryStatus, SaveMemoryInput, build_explicit_memory_record,
    build_saved_memory, capture_memory,
};
pub use privacy::{redact_memory_metadata, redact_sensitive_content, redact_work_context};
pub use projects::{
    Project, ProjectAttachment, ProjectBranch, ProjectEntity, ProjectEntityInput,
    ProjectFreshnessEvent, ProjectMaintenanceReport, ProjectMergeResult, ProjectWorktree,
    publication_fingerprint,
};
pub use recall::{
    CodeRelationship, ContextPack, RecallFilter, RecallResult, build_context_pack,
    memory_matches_filter, prefer_decision, prefer_newer_contradiction, recall_result,
};
pub use sessions::{Session, carry_summary_state, derive_session_memory, session_memory_id};
pub use sharing::{ProjectConflict, ProjectPerson, competing_project_memories};
pub use store::MemoryStore;
pub use sync::{SyncDeletion, SyncDiagnostics, SyncEvent, SyncPullResult, SyncPushResult};
pub use transfer::{
    EXPORT_FORMAT, EXPORT_VERSION, ExportBundle, ImportResult, normalize_export_bundle,
};
pub use types::{StoreError, WorkContext, now_seconds};

pub use embeddings::{
    EMBEDDING_DIMENSIONS, EMBEDDING_MODEL, EMBEDDING_VERSION, MemoryEmbedding,
    embedding_fingerprint, embedding_text, validate_vector,
};
pub use hybrid::{SEMANTIC_CANDIDATES, hybrid_rank};
pub use retention::{
    ACCESS_SATURATION, ACCESS_WEIGHT, AGE_HALF_LIFE_DAYS, AGE_WEIGHT, BELOW_THRESHOLD_FLOOR,
    FEEDBACK_CAP, FEEDBACK_STEP, FeedbackVote, MemoryFeedback, RECENCY_HALF_LIFE_DAYS,
    RECENCY_WEIGHT, RETENTION_THRESHOLD, SALIENCE_WEIGHT, apply_feedback, decays, feedback_factor,
    ranking_multiplier, retention_factor, retention_score, sync_feedback_counts,
};
