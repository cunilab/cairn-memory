use crate::AppState;
use crate::auth::Web;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::State;
use cairn_core::account::Device;
use cairn_store::device::DeviceRegistration;
use serde::Deserialize;

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

pub async fn list(State(state): State<AppState>, Web(account): Web) -> ApiResult<Vec<Device>> {
    Ok(Json(state.store.list_devices(&account.person_id).await?))
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
