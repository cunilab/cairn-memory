use crate::lifecycle::{Lifecycle, LifecycleInput, MemoryScope, validate_lifecycle_input};
use crate::privacy::{redact_sensitive_content, redact_work_context};
use crate::store::MemoryStore;
use crate::types::{StoreError, WorkContext, timestamp};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_OBSERVATION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservationInput {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    pub importance: Option<String>,
    #[serde(default)]
    pub lifecycle: LifecycleInput,
    #[serde(default)]
    pub context: WorkContext,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    pub importance: Option<String>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub scope: MemoryScope,
    pub context: WorkContext,
    pub captured_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureResult {
    #[serde(default)]
    pub project_attachment: Option<crate::projects::ProjectAttachment>,
    pub accepted: bool,
    pub duplicate: bool,
    pub observation: Observation,
}

impl MemoryStore {
    pub(crate) fn stage_observation(
        &mut self,
        person_id: Option<&str>,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        validate_observation(&input)?;

        if let Some(existing) = self
            .state
            .observations
            .iter()
            .find(|observation| observation.event_id == input.event_id)
        {
            if !is_replay(existing, &input) {
                return Err(changed_replay(&input.event_id));
            }
            return Ok(CaptureResult {
                project_attachment: None,
                accepted: true,
                duplicate: true,
                observation: existing.clone(),
            });
        }

        // A bare project id (the CLI sends only that) still inherits the session's context.
        let bare = WorkContext {
            project_id: None,
            ..input.context.clone()
        } == WorkContext::default();
        let mut context = if bare {
            self.state
                .sessions
                .iter()
                .find(|session| {
                    session.session_id == input.session_id
                        && session.person_id.as_deref() == person_id
                })
                .map(|session| session.context.clone())
                .unwrap_or_default()
        } else {
            input.context.clone()
        };
        context.project_id = input.context.project_id.clone();
        let mut normalized_input = input;
        normalized_input.context = context;
        let observation = normalize_observation(&normalized_input)?;
        self.state.observations.push(observation.clone());
        Ok(CaptureResult {
            project_attachment: None,
            accepted: true,
            duplicate: false,
            observation,
        })
    }

    pub(crate) fn unstage_observation(&mut self, event_id: &str) {
        self.state
            .observations
            .retain(|observation| observation.event_id != event_id);
    }

    pub(crate) fn unstage_observation_owner(&mut self, event_id: &str) {
        self.state
            .observation_owners
            .retain(|record| record.event_id != event_id);
    }

    pub fn capture_observation(
        &mut self,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        let result = self.stage_observation(None, input)?;
        if result.duplicate {
            return Ok(result);
        }
        let event_id = result.observation.event_id.clone();
        if let Err(error) = self.persist() {
            // The rename did not replace the file. Drop the staged row so a retry writes.
            self.unstage_observation(&event_id);
            return Err(error);
        }
        Ok(result)
    }

    pub fn capture_for_account(
        &mut self,
        token: &str,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        let account = self.authenticate_account(token)?;
        let mut result = self.capture_for_account_inner(token, input)?;
        result.project_attachment =
            Some(self.attach_context(&account.person_id, &result.observation.context));
        Ok(result)
    }

    fn capture_for_account_inner(
        &mut self,
        account_token: &str,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        let account = self.authenticate_account(account_token)?;
        // Auth before the event id. Do not return or claim a foreign or unowned row.
        validate_observation(&input)?;
        let person_id = account.person_id.clone();
        if self.id_owned_by_other(&input.event_id, &person_id) {
            return Err(StoreError::Validation(format!(
                "event belongs to another person: {}",
                input.event_id
            )));
        }
        if self
            .state
            .observations
            .iter()
            .any(|observation| observation.event_id == input.event_id)
            && !self.observation_owned_by(&input.event_id, &person_id)
        {
            let owned_by_someone_else = self
                .state
                .observation_owners
                .iter()
                .any(|record| record.event_id == input.event_id);
            return Err(if owned_by_someone_else {
                StoreError::Validation(format!(
                    "event belongs to another person: {}",
                    input.event_id
                ))
            } else {
                outside_sync_scope(&input.event_id)
            });
        }
        let result = self.stage_observation(Some(person_id.as_str()), input)?;
        if result.duplicate {
            return Ok(result);
        }
        let event_id = result.observation.event_id.clone();
        self.record_observation_owner(&person_id, &event_id);
        if let Err(error) = self.persist() {
            // Observation and owner land in one rename. A failed rename leaves neither.
            self.unstage_observation(&event_id);
            self.unstage_observation_owner(&event_id);
            return Err(error);
        }
        // The capture is acknowledged. Housekeeping failures must not turn it into an error.
        self.finish_idle_sessions(&person_id);
        Ok(result)
    }
}

pub fn normalize_observation(input: &ObservationInput) -> Result<Observation, StoreError> {
    validate_observation(input)?;
    let lifecycle = Lifecycle::observation(
        &input.lifecycle,
        input.importance.as_deref(),
        &input.content,
        &input.context,
    );
    Ok(Observation {
        event_id: input.event_id.clone(),
        session_id: input.session_id.clone(),
        content: redact_sensitive_content(&input.content),
        role: input.role.clone(),
        source: input.source.clone(),
        importance: input.importance.clone(),
        lifecycle,
        scope: MemoryScope::Session,
        context: redact_work_context(&input.context),
        captured_at: timestamp(),
    })
}

/// A retry is the same event only if what identifies its payload is unchanged.
pub(crate) fn is_replay(stored: &Observation, input: &ObservationInput) -> bool {
    stored.session_id == input.session_id
        && stored.content == redact_sensitive_content(&input.content)
        && stored.role == input.role
        && stored.source == input.source
}

pub(crate) fn changed_replay(event_id: &str) -> StoreError {
    StoreError::Conflict(format!(
        "event id already used with different content: {event_id}"
    ))
}

pub(crate) fn outside_sync_scope(event_id: &str) -> StoreError {
    StoreError::Conflict(format!(
        "event already exists outside sync scope: {event_id}"
    ))
}

pub(crate) fn validate_observation(input: &ObservationInput) -> Result<(), StoreError> {
    for (name, value) in [
        ("event_id", input.event_id.as_str()),
        ("session_id", input.session_id.as_str()),
        ("content", input.content.as_str()),
        ("role", input.role.as_str()),
        ("source", input.source.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::Validation(format!("{name} must not be empty")));
        }
    }
    if input.content.len() > MAX_OBSERVATION_BYTES {
        return Err(StoreError::Validation(format!(
            "content exceeds the {} byte limit",
            MAX_OBSERVATION_BYTES
        )));
    }
    validate_lifecycle_input(&input.lifecycle)?;
    Ok(())
}

#[cfg(test)]
mod tests;
