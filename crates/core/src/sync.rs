use crate::capture::{
    Observation, ObservationInput, changed_replay, is_replay, outside_sync_scope,
};
use crate::sessions::Session;
use crate::store::MemoryStore;
use crate::types::{StoreError, WorkContext, timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncEvent {
    pub cursor: u64,
    pub event_id: String,
    pub person_id: String,
    pub device_id: String,
    pub observation: Observation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPushResult {
    pub accepted: bool,
    pub duplicate: bool,
    pub cursor: u64,
    pub observation: Observation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncDeletion {
    pub cursor: u64,
    pub item_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPullResult {
    pub cursor: u64,
    pub events: Vec<SyncEvent>,
    /// Deletion instructions for items this device may already have stored.
    /// A device that never pulls again is left unchanged.
    #[serde(default)]
    pub deletions: Vec<SyncDeletion>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncDiagnostics {
    pub device_id: String,
    pub device_cursor: u64,
    pub server_cursor: u64,
    pub lag: u64,
    pub pending_events: usize,
    pub rejected_events: usize,
    pub status: String,
    #[serde(default)]
    pub recovery_hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SyncDeletionRecord {
    pub(crate) cursor: u64,
    pub(crate) person_id: String,
    pub(crate) item_id: String,
}

impl MemoryStore {
    pub fn sync_push(
        &mut self,
        auth_token: &str,
        input: ObservationInput,
    ) -> Result<SyncPushResult, StoreError> {
        let (device, previous_seen) = self.authenticate_device_in_memory(auth_token)?;
        if self.id_owned_by_other(&input.event_id, &device.person_id) {
            self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
            return Err(StoreError::Validation(format!(
                "event belongs to another person: {}",
                input.event_id
            )));
        }
        if let Some(existing) = self
            .state
            .observations
            .iter()
            .find(|observation| observation.event_id == input.event_id)
            .cloned()
        {
            if let Some((cursor, person_id)) = self
                .state
                .sync_events
                .iter()
                .find(|event| event.event_id == existing.event_id)
                .map(|event| (event.cursor, event.person_id.clone()))
            {
                // A duplicate is a persisted sync event for this person, not the event id alone.
                let foreign = person_id != device.person_id;
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                if foreign {
                    return Err(StoreError::Validation(format!(
                        "event belongs to another person: {}",
                        existing.event_id
                    )));
                }
                if !is_replay(&existing, &input) {
                    return Err(changed_replay(&input.event_id));
                }
                return Ok(SyncPushResult {
                    accepted: true,
                    duplicate: true,
                    cursor,
                    observation: existing,
                });
            }

            let event_id = existing.event_id.clone();
            if !self.observation_owned_by(&event_id, &device.person_id) {
                let foreign_owner = self
                    .state
                    .observation_owners
                    .iter()
                    .any(|record| record.event_id == event_id);
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                return Err(if foreign_owner {
                    StoreError::Validation(format!("event belongs to another person: {event_id}"))
                } else {
                    outside_sync_scope(&event_id)
                });
            }

            // Attach the stored observation, but only for the same event: changed text conflicts.
            if !is_replay(&existing, &input) {
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                return Err(changed_replay(&event_id));
            }
            let previous_cursor = self.state.next_sync_cursor;
            self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
            let cursor = self.state.next_sync_cursor;
            self.state.sync_events.push(SyncEvent {
                cursor,
                event_id: event_id.clone(),
                person_id: device.person_id.clone(),
                device_id: device.device_id.clone(),
                observation: existing.clone(),
            });
            if let Err(error) = self.persist() {
                // Drop the new sync row only. The stored observation and owner stay.
                self.state
                    .sync_events
                    .retain(|event| event.event_id != event_id);
                self.state.next_sync_cursor = previous_cursor;
                self.restore_device_seen(&device.device_id, &previous_seen);
                return Err(error);
            }
            return Ok(SyncPushResult {
                accepted: true,
                duplicate: false,
                cursor,
                observation: existing,
            });
        }

        let capture = match self.stage_observation(Some(device.person_id.as_str()), input) {
            Ok(capture) => capture,
            Err(error) => {
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                return Err(error);
            }
        };

        let event_id = capture.observation.event_id.clone();
        // The device person owns the new observation, so the account sees it like an observe.
        self.record_observation_owner(&device.person_id, &event_id);
        // The hook never starts a session, so a push creates the row from the pushed context
        // (as on PostgreSQL). An empty row takes the first real context; a real one is kept.
        let session_id = capture.observation.session_id.clone();
        let pushed_context = capture.observation.context.clone();
        let mut created_session = false;
        let mut filled_session = None;
        if let Some(session) = self.state.sessions.iter_mut().find(|session| {
            session.session_id == session_id
                && session.person_id.as_deref() == Some(device.person_id.as_str())
        }) {
            if session.context == WorkContext::default() && pushed_context != WorkContext::default()
            {
                filled_session = Some(std::mem::replace(&mut session.context, pushed_context));
            }
        } else {
            created_session = true;
            self.state.sessions.push(Session {
                session_id: session_id.clone(),
                person_id: Some(device.person_id.clone()),
                context: pushed_context,
                started_at: timestamp(),
                ended_at: None,
            });
        }
        let previous_cursor = self.state.next_sync_cursor;
        self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
        let cursor = self.state.next_sync_cursor;
        self.state.sync_events.push(SyncEvent {
            cursor,
            event_id: event_id.clone(),
            person_id: device.person_id.clone(),
            device_id: device.device_id.clone(),
            observation: capture.observation.clone(),
        });
        if let Err(error) = self.persist() {
            // Observation and sync event land in one rename. A failed rename leaves neither.
            self.state
                .sync_events
                .retain(|event| event.event_id != event_id);
            self.state.next_sync_cursor = previous_cursor;
            self.unstage_observation(&event_id);
            self.unstage_observation_owner(&event_id);
            if created_session {
                self.state.sessions.retain(|session| {
                    session.session_id != session_id
                        || session.person_id.as_deref() != Some(device.person_id.as_str())
                });
            }
            if let Some(previous) = filled_session
                && let Some(session) = self.state.sessions.iter_mut().find(|session| {
                    session.session_id == session_id
                        && session.person_id.as_deref() == Some(device.person_id.as_str())
                })
            {
                session.context = previous;
            }
            self.restore_device_seen(&device.device_id, &previous_seen);
            return Err(error);
        }
        // The push is acknowledged. Housekeeping failures must not turn it into an error.
        if let Err(error) =
            self.auto_register_project(&device.person_id, &capture.observation.context)
        {
            eprintln!("cairn: automatic project registration failed: {error}");
        }
        self.finish_idle_sessions(&device.person_id);
        Ok(SyncPushResult {
            accepted: true,
            duplicate: false,
            cursor,
            observation: capture.observation,
        })
    }

    pub fn sync_pull(
        &mut self,
        auth_token: &str,
        cursor: u64,
    ) -> Result<SyncPullResult, StoreError> {
        let device = self.authenticate_device(auth_token)?;
        let events: Vec<SyncEvent> = self
            .state
            .sync_events
            .iter()
            .filter(|event| {
                event.person_id == device.person_id
                    && event.cursor > cursor
                    && !self.state.forgotten_ids.contains(&event.event_id)
            })
            .cloned()
            .collect();
        let deletions: Vec<SyncDeletion> = self
            .state
            .sync_deletions
            .iter()
            .filter(|deletion| deletion.person_id == device.person_id && deletion.cursor > cursor)
            .map(|deletion| SyncDeletion {
                cursor: deletion.cursor,
                item_id: deletion.item_id.clone(),
            })
            .collect();
        let next_cursor = events
            .iter()
            .map(|event| event.cursor)
            .chain(deletions.iter().map(|deletion| deletion.cursor))
            .max()
            .unwrap_or(cursor.max(device.cursor));
        if next_cursor > device.cursor
            && let Some(record) = self
                .state
                .devices
                .iter_mut()
                .find(|record| record.device.device_id == device.device_id)
        {
            record.device.cursor = next_cursor;
            self.persist()?;
        }
        Ok(SyncPullResult {
            cursor: next_cursor,
            events,
            deletions,
        })
    }

    pub fn sync_diagnostics(&mut self, auth_token: &str) -> Result<SyncDiagnostics, StoreError> {
        let device = self.authenticate_device(auth_token)?;
        let server_cursor = self
            .state
            .sync_events
            .iter()
            .filter(|event| event.person_id == device.person_id)
            .map(|event| event.cursor)
            .max()
            .unwrap_or(0);
        let lag = server_cursor.saturating_sub(device.cursor);
        Ok(SyncDiagnostics {
            device_id: device.device_id,
            device_cursor: device.cursor,
            server_cursor,
            lag,
            pending_events: 0,
            rejected_events: 0,
            status: if lag == 0 { "caught_up" } else { "behind" }.to_owned(),
            recovery_hint: (lag > 0).then(|| {
                "Pull from the last persisted cursor to resume synchronization.".to_owned()
            }),
        })
    }
}

#[cfg(test)]
mod tests;
