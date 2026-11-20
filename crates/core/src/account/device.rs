use serde::Serialize;

/// Every device token starts with this, so a caller can skip lookups for other credentials.
pub const DEVICE_TOKEN_PREFIX: &str = "cairn-device-";

/// Device use is recorded at most this often, so read-only calls do not churn rows.
pub const LAST_SEEN_THROTTLE_SECONDS: i64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Active,
    Revoked,
}

impl DeviceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Device {
    pub device_id: String,
    pub person_id: String,
    pub name: String,
    pub status: DeviceStatus,
    pub created_at: i64,
    pub last_seen_at: i64,
}
