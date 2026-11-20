use crate::legacy::accounts::{AccountStatus, hash_token, random_token};
use crate::legacy::store::MemoryStore;
use crate::legacy::types::{StoreError, now_seconds, timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    #[default]
    Active,
    Revoked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub device_id: String,
    pub person_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub status: DeviceStatus,
    pub registered_at: String,
    pub last_seen_at: String,
    pub cursor: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRegistration {
    pub device: Device,
    pub token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DeviceRecord {
    pub(crate) device: Device,
    pub(crate) token_hash: String,
}

impl MemoryStore {
    pub fn register_device(
        &mut self,
        device_id: impl Into<String>,
        person_id: impl Into<String>,
        agent_id: Option<String>,
        name: impl Into<String>,
    ) -> Result<DeviceRegistration, StoreError> {
        let device_id = device_id.into();
        let person_id = person_id.into();
        let name = name.into();
        for (field, value) in [
            ("device_id", device_id.as_str()),
            ("person_id", person_id.as_str()),
            ("device name", name.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(StoreError::Validation(format!("{field} must not be empty")));
            }
        }
        if let Some(existing) = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == device_id)
        {
            if existing.device.person_id != person_id {
                return Err(StoreError::Validation(format!(
                    "device already belongs to another person: {device_id}"
                )));
            }
            if existing.device.status == DeviceStatus::Revoked {
                return Err(StoreError::Validation(
                    "device is revoked; register a new device id".to_owned(),
                ));
            }
            // The token is shown once and stored hashed, so a repeat registration issues a new one.
            let token = device_token(&device_id);
            existing.token_hash = hash_token(&token);
            existing.device.last_seen_at = timestamp();
            let device = existing.device.clone();
            self.persist()?;
            return Ok(DeviceRegistration { device, token });
        }

        let now = timestamp();
        let device = Device {
            device_id,
            person_id,
            agent_id,
            name,
            status: DeviceStatus::Active,
            registered_at: now.clone(),
            last_seen_at: now,
            cursor: 0,
        };
        let token = device_token(&device.device_id);
        self.state.devices.push(DeviceRecord {
            device: device.clone(),
            token_hash: hash_token(&token),
        });
        self.persist()?;
        Ok(DeviceRegistration { device, token })
    }

    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.state.devices.iter().map(|record| &record.device)
    }

    pub(crate) fn authenticate_device_in_memory(
        &mut self,
        token: &str,
    ) -> Result<(Device, String), StoreError> {
        let token_hash = hash_token(token);
        let record = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.token_hash == token_hash)
            .ok_or_else(|| StoreError::Validation("device authentication failed".to_owned()))?;
        if record.device.status != DeviceStatus::Active {
            return Err(StoreError::Validation("device is revoked".to_owned()));
        }
        if !self.state.accounts.iter().any(|candidate| {
            candidate.account.person_id == record.device.person_id
                && candidate.account.status == AccountStatus::Active
        }) {
            return Err(StoreError::Validation(
                "account authentication failed".to_owned(),
            ));
        }
        let previous_seen = record.device.last_seen_at.clone();
        touch_last_seen(&mut record.device);
        let device = record.device.clone();
        Ok((device, previous_seen))
    }

    pub(crate) fn restore_device_seen(&mut self, device_id: &str, previous_seen: &str) {
        if let Some(record) = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == device_id)
        {
            record.device.last_seen_at = previous_seen.to_owned();
        }
    }

    pub(crate) fn persist_or_restore_seen(
        &mut self,
        device_id: &str,
        previous_seen: &str,
    ) -> Result<(), StoreError> {
        if let Err(error) = self.persist() {
            self.restore_device_seen(device_id, previous_seen);
            return Err(error);
        }
        Ok(())
    }

    /// Read-only sync authentication: last-seen stays in memory until a write persists it.
    pub fn authenticate_device(&mut self, token: &str) -> Result<Device, StoreError> {
        let (device, _) = self.authenticate_device_in_memory(token)?;
        Ok(device)
    }

    pub fn revoke_device(
        &mut self,
        auth_token: &str,
        target_device_id: &str,
    ) -> Result<Device, StoreError> {
        let caller = self.authenticate_session(auth_token)?;
        let target = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == target_device_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown device: {target_device_id}")))?;
        if caller.person_id != target.device.person_id {
            return Err(StoreError::Validation(
                "device is outside the caller scope".to_owned(),
            ));
        }
        target.device.status = DeviceStatus::Revoked;
        let device = target.device.clone();
        self.persist()?;
        Ok(device)
    }

    pub fn rotate_device_token(
        &mut self,
        auth_token: &str,
        target_device_id: &str,
    ) -> Result<DeviceRegistration, StoreError> {
        let caller = self.authenticate_session(auth_token)?;
        let target = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == target_device_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown device: {target_device_id}")))?;
        if caller.person_id != target.device.person_id {
            return Err(StoreError::Validation(
                "device is outside the caller scope".to_owned(),
            ));
        }
        let token = device_token(&target.device.device_id);
        target.token_hash = hash_token(&token);
        target.device.last_seen_at = timestamp();
        let device = target.device.clone();
        self.persist()?;
        Ok(DeviceRegistration { device, token })
    }
}

/// Device use is recorded at most this often, so read-only calls do not churn state.
const LAST_SEEN_THROTTLE_SECS: u64 = 60;

pub(crate) fn touch_last_seen(device: &mut Device) {
    let now = now_seconds();
    let stale = device.last_seen_at.parse::<u64>().map_or(true, |seen| {
        now.saturating_sub(seen) >= LAST_SEEN_THROTTLE_SECS
    });
    if stale {
        device.last_seen_at = now.to_string();
    }
}

/// Every device token starts with this, so a caller can skip lookups for other credentials.
pub const DEVICE_TOKEN_PREFIX: &str = "cairn-device-";

fn device_token(_device_id: &str) -> String {
    random_token("cairn-device")
}

#[cfg(test)]
mod tests;
