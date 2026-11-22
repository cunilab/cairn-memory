use crate::auth::Web;
use crate::error::{ApiError, ApiResult};
use crate::{AppState, VERSION};
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use cairn_core::account::{Device, DeviceHealth, device_health};
use cairn_core::id::now_seconds;
use cairn_store::device::DeviceRegistration;
use serde::Deserialize;
use serde::Serialize;

#[derive(Deserialize)]
pub struct Register {
    #[serde(rename = "deviceId", alias = "device_id", default)]
    device_id: Option<String>,
    /// Defaults to the signed-in account; any other value is refused.
    #[serde(rename = "personId", alias = "person_id", default)]
    person_id: Option<String>,
    name: String,
}

#[derive(Deserialize)]
pub struct Target {
    #[serde(rename = "deviceId", alias = "device_id")]
    device_id: String,
}

/// A device with what the console says about it. A revoked device has no health.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct DeviceView {
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    device: Device,
    health: Option<DeviceHealth>,
    health_reason: Option<String>,
    fix_command: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Web(account): Web,
    headers: HeaderMap,
) -> ApiResult<Vec<DeviceView>> {
    let public_url = state.public_url(&headers);
    let now = now_seconds();
    Ok(Json(
        state
            .store
            .list_devices(&account.person_id)
            .await?
            .into_iter()
            .map(|device| {
                let report = device_health(&device, VERSION, &public_url, now);
                DeviceView {
                    health: report.as_ref().map(|r| r.health),
                    health_reason: report.as_ref().and_then(|r| r.reason.clone()),
                    fix_command: report.and_then(|r| r.fix_command),
                    device,
                }
            })
            .collect(),
    ))
}

pub async fn register(
    State(state): State<AppState>,
    Web(account): Web,
    Json(body): Json<Register>,
) -> ApiResult<DeviceRegistration> {
    if body.person_id.is_some_and(|p| p != account.person_id) {
        return Err(ApiError::forbidden("device is outside the caller scope"));
    }
    let registration = state
        .store
        .register_device(&account.person_id, body.device_id.as_deref(), &body.name)
        .await?;
    state
        .store
        .record_audit(
            &account.person_id,
            Some(&registration.device.device_id),
            "device_registered",
            Some(&registration.device.device_id),
            serde_json::json!({}),
        )
        .await?;
    Ok(Json(registration))
}

pub async fn revoke(
    State(state): State<AppState>,
    Web(account): Web,
    Json(body): Json<Target>,
) -> ApiResult<Device> {
    state
        .store
        .revoke_device(&account.person_id, &body.device_id)
        .await?;
    state
        .store
        .record_audit(
            &account.person_id,
            Some(&body.device_id),
            "device_revoked",
            Some(&body.device_id),
            serde_json::json!({}),
        )
        .await?;
    let devices = state.store.list_devices(&account.person_id).await?;
    devices
        .into_iter()
        .find(|d| d.device_id == body.device_id)
        .map(Json)
        .ok_or_else(|| ApiError::invalid("unknown device"))
}

pub async fn rotate(
    State(state): State<AppState>,
    Web(account): Web,
    Json(body): Json<Target>,
) -> ApiResult<DeviceRegistration> {
    let registration = state
        .store
        .rotate_device(&account.person_id, &body.device_id)
        .await?;
    state
        .store
        .record_audit(
            &account.person_id,
            Some(&body.device_id),
            "device_rotated",
            Some(&body.device_id),
            serde_json::json!({}),
        )
        .await?;
    Ok(Json(registration))
}
