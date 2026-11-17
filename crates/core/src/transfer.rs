use crate::capture::{MAX_OBSERVATION_BYTES, Observation};
use crate::lifecycle::{sweep_expired_state, validate_lifecycle};
use crate::memories::{MAX_MEMORY_BYTES, Memory};
use crate::privacy::{redact_memory_metadata, redact_sensitive_content, redact_work_context};
use crate::sessions::Session;
use crate::store::{MemoryStore, StoreState};
use crate::types::{StoreError, timestamp};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const EXPORT_FORMAT: &str = "cairn-memory";

pub const EXPORT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportBundle {
    pub format: String,
    pub version: u32,
    #[serde(rename = "exportedAt")]
    pub exported_at: String,
    pub sessions: Vec<Session>,
    pub observations: Vec<Observation>,
    pub memories: Vec<Memory>,
    #[serde(rename = "forgottenIds", default)]
    pub forgotten_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportResult {
    pub imported_sessions: usize,
    pub imported_observations: usize,
    pub imported_memories: usize,
    pub imported_forgotten_ids: usize,
    /// Links removed because link creation would reject them.
    #[serde(default)]
    pub dropped_links: Vec<crate::DroppedLink>,
}

impl MemoryStore {
    pub fn export(&self) -> ExportBundle {
        self.export_owned(None)
    }

    pub fn export_for_account(&self, account_token: &str) -> Result<ExportBundle, StoreError> {
        let account = self.authenticate_account(account_token)?;
        Ok(self.export_owned(Some(account.person_id.as_str())))
    }

    fn export_owned(&self, person_id: Option<&str>) -> ExportBundle {
        let observations = self
            .state
            .observations
            .iter()
            .filter(|observation| {
                person_id.is_none_or(|person_id| {
                    self.observation_owned_by(&observation.event_id, person_id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let memories = self
            .state
            .memories
            .iter()
            .filter(|memory| {
                person_id.is_none_or(|person_id| {
                    memory.scope == crate::MemoryScope::Personal
                        && memory.published_by.is_none()
                        && self.memory_owned_by(&memory.memory_id, person_id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let sessions = self
            .state
            .sessions
            .iter()
            .filter(|session| session.person_id.as_deref() == person_id)
            .cloned()
            .collect();
        let mut forgotten_ids = self
            .state
            .forgotten_ids
            .iter()
            .filter(|id| {
                person_id.is_none_or(|person_id| {
                    self.observation_owned_by(id, person_id)
                        || self.memory_owned_by(id, person_id)
                        || memories.iter().any(|memory| {
                            memory.source_event_ids.iter().any(|source| source == *id)
                        })
                        || observations
                            .iter()
                            .any(|observation| observation.event_id == **id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        forgotten_ids.sort();
        ExportBundle {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at: timestamp(),
            sessions,
            observations,
            memories,
            forgotten_ids,
        }
    }

    pub fn export_json(&self) -> Result<String, StoreError> {
        Ok(serde_json::to_string_pretty(&self.export())?)
    }

    pub fn import_json(&mut self, raw: &str) -> Result<ImportResult, StoreError> {
        let bundle: ExportBundle = serde_json::from_str(raw)?;
        self.import_bundle(bundle)
    }

    pub fn import_for_account(
        &mut self,
        account_token: &str,
        mut bundle: ExportBundle,
    ) -> Result<ImportResult, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let person_id = account.person_id.clone();
        for observation in &bundle.observations {
            if self
                .item_owner(&observation.event_id)
                .is_some_and(|owner| owner != person_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
        }
        for memory in &bundle.memories {
            if self
                .item_owner(&memory.memory_id)
                .is_some_and(|owner| owner != person_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
        }
        for memory in &mut bundle.memories {
            memory.fields.author = Some(person_id.clone());
            memory.published_by = None;
            memory.scope = crate::MemoryScope::Personal;
            if memory.changed_by.is_some() {
                memory.changed_by = Some(person_id.clone());
            }
            memory.project_id = None;
            memory.promoted_from = None;
            memory.promotion_reason = None;
        }
        let observation_ids = bundle
            .observations
            .iter()
            .map(|observation| observation.event_id.clone())
            .collect::<Vec<_>>();
        let memory_ids = bundle
            .memories
            .iter()
            .map(|memory| memory.memory_id.clone())
            .collect::<Vec<_>>();
        // A tombstone applies only to an id this person owns or brings as a row. A foreign
        // id rejects the import with the row-ownership error (no payload or owner), which
        // does reveal that the id is owned; an unknown id is ignored. A source id is not
        // proof, since a row can name a foreign or future id as its source. A caller can
        // still bring a row under an unused id and so reserve it; capture and sync push
        // then reject that id loudly (see `id_owned_by_other`).
        let brought = observation_ids
            .iter()
            .chain(&memory_ids)
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut applied = Vec::with_capacity(bundle.forgotten_ids.len());
        for id in std::mem::take(&mut bundle.forgotten_ids) {
            match self.item_owner(&id) {
                Some(owner) if owner != person_id => {
                    return Err(StoreError::Validation(
                        "account does not own the memory".to_owned(),
                    ));
                }
                Some(_) => applied.push(id),
                None if brought.contains(id.as_str()) => applied.push(id),
                None => {}
            }
        }
        bundle.forgotten_ids = applied;
        for session in &mut bundle.sessions {
            if session
                .person_id
                .as_deref()
                .is_some_and(|owner| owner != person_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
            // An old bundle has no person. Bind it to the caller so it does not become the unscoped row.
            session.person_id = Some(person_id.clone());
        }
        // One persist for rows, tombstones, expiry tombstones and owners: a failure restores
        // the prior state, so no row is left in memory or on disk without its owner.
        let readable = self.memories_for_account(account_token)?;
        let (candidate, result, _) = self.stage_import(bundle, &readable)?;
        self.state = candidate;
        for event_id in observation_ids {
            self.record_observation_owner(&person_id, &event_id);
        }
        for memory_id in memory_ids {
            self.record_memory_owner(&person_id, &memory_id);
        }
        self.persist()?;
        Ok(result)
    }

    pub fn import_bundle(&mut self, bundle: ExportBundle) -> Result<ImportResult, StoreError> {
        let (candidate, result, changed) = self.stage_import(bundle, &self.state.memories)?;
        if changed {
            self.state = candidate;
            self.persist()?;
        }
        Ok(result)
    }

    /// Merges a bundle into a clone of the state, with expiry tombstones when anything changed.
    /// Nothing is written; the caller commits the candidate.
    fn stage_import(
        &self,
        mut bundle: ExportBundle,
        readable: &[crate::Memory],
    ) -> Result<(StoreState, ImportResult, bool), StoreError> {
        normalize_export_bundle(&mut bundle)?;
        let dropped_links = crate::sanitize_imported_links(&mut bundle.memories, readable);
        let mut candidate = self.state.clone();
        let mut result = ImportResult {
            imported_sessions: 0,
            imported_observations: 0,
            imported_memories: 0,
            imported_forgotten_ids: 0,
            dropped_links,
        };

        for session in bundle.sessions {
            if let Some(existing) = candidate.sessions.iter().find(|existing| {
                existing.session_id == session.session_id && existing.person_id == session.person_id
            }) {
                if existing != &session {
                    return Err(import_conflict("session", &session.session_id));
                }
            } else {
                candidate.sessions.push(session);
                result.imported_sessions += 1;
            }
        }

        for observation in bundle.observations {
            if let Some(existing) = candidate
                .observations
                .iter()
                .find(|existing| existing.event_id == observation.event_id)
            {
                if existing != &observation {
                    return Err(import_conflict("observation", &observation.event_id));
                }
            } else {
                candidate.observations.push(observation);
                result.imported_observations += 1;
            }
        }

        for memory in bundle.memories {
            if let Some(existing) = candidate
                .memories
                .iter()
                .find(|existing| existing.memory_id == memory.memory_id)
            {
                if existing != &memory {
                    return Err(import_conflict("memory", &memory.memory_id));
                }
            } else {
                candidate.memories.push(memory);
                result.imported_memories += 1;
            }
        }

        for forgotten_id in bundle.forgotten_ids {
            if candidate.forgotten_ids.contains(&forgotten_id) {
                continue;
            }
            candidate.forgotten_ids.insert(forgotten_id);
            result.imported_forgotten_ids += 1;
        }

        let changed = result.imported_sessions > 0
            || result.imported_observations > 0
            || result.imported_memories > 0
            || result.imported_forgotten_ids > 0;
        if changed {
            sweep_expired_state(&mut candidate);
        }
        Ok((candidate, result, changed))
    }
}

pub fn normalize_export_bundle(bundle: &mut ExportBundle) -> Result<(), StoreError> {
    if bundle.format != EXPORT_FORMAT {
        return Err(StoreError::Validation(format!(
            "unsupported export format {}; expected {EXPORT_FORMAT}",
            bundle.format
        )));
    }
    if bundle.version != EXPORT_VERSION {
        return Err(StoreError::Validation(format!(
            "unsupported export version {}; expected {EXPORT_VERSION}",
            bundle.version
        )));
    }
    for session in &mut bundle.sessions {
        if session.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export session_id must not be empty".to_owned(),
            ));
        }
        session.context = redact_work_context(&session.context);
    }
    for observation in &mut bundle.observations {
        observation.scope = crate::MemoryScope::Personal;
        if observation.event_id.trim().is_empty() || observation.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export observation ids must not be empty".to_owned(),
            ));
        }
        if observation.content.len() > MAX_OBSERVATION_BYTES {
            return Err(StoreError::Validation(format!(
                "export observation exceeds the {} byte limit",
                MAX_OBSERVATION_BYTES
            )));
        }
        observation.content = redact_sensitive_content(&observation.content);
        observation.context = redact_work_context(&observation.context);
        validate_lifecycle(&observation.lifecycle)?;
    }
    for memory in &mut bundle.memories {
        memory.feedback.clear();
        crate::sync_feedback_counts(memory);
        memory.published_by = None;
        memory.scope = crate::MemoryScope::Personal;
        memory.project_id = None;
        memory.promoted_from = None;
        memory.promotion_reason = None;
        if memory.memory_id.trim().is_empty() || memory.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export memory ids must not be empty".to_owned(),
            ));
        }
        if memory.content.len() > MAX_MEMORY_BYTES {
            return Err(StoreError::Validation(format!(
                "export memory exceeds the {} byte limit",
                MAX_MEMORY_BYTES
            )));
        }
        memory.content = redact_sensitive_content(&memory.content);
        redact_memory_metadata(memory);
        validate_lifecycle(&memory.lifecycle)?;
    }
    bundle.forgotten_ids.retain(|id| !id.trim().is_empty());
    bundle.forgotten_ids.sort();
    bundle.forgotten_ids.dedup();
    Ok(())
}

fn import_conflict(kind: &str, id: &str) -> StoreError {
    StoreError::Validation(format!("conflicting {kind} during import: {id}"))
}

#[cfg(test)]
mod tests;
