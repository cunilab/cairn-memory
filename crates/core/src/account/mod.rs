//! Accounts, devices and audit. Pure primitives only; storage lives in `cairn-store`.

pub mod audit;
pub mod auth;
pub mod device;

pub use audit::{AuditEvent, SYSTEM_ACTIONS};
pub use auth::{
    ADMIN_ONLY_ERROR, AUTH_SESSION_SECONDS, Account, AccountStatus, WEB_ONLY_ERROR, hash_password,
    hash_token, normalize_email, random_token, validate_password, verify_password,
};
pub use device::{
    AGENTS, ClientInfo, ClientKind, DEVICE_TOKEN_PREFIX, Device, DeviceHealth, DeviceStatus,
    HealthReport, client_user_agent, device_health, parse_user_agent, version_older,
};
