use crate::capture::Observation;
use crate::privacy::{contains_private_key_header, find_token_prefix, human_context_texts};
use crate::sessions::keeps_expired_source;
use crate::store::{MemoryStore, StoreState};
use crate::sync::SyncDeletionRecord;
use crate::types::{StoreError, WorkContext, now_seconds};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const DEFAULT_TEMPORARY_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    #[default]
    Private,
    Sensitive,
    Secret,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    #[default]
    Temporary,
    Durable,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    SessionSummary,
    Handoff,
    #[default]
    Explicit,
    Lesson,
    Correction,
    Decision,
    Rule,
    Gotcha,
    Procedure,
    Fact,
    Preference,
    Capture,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    #[default]
    Personal,
    Project,
    Global,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleInput {
    #[serde(default)]
    pub importance: Option<u8>,
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
    #[serde(default)]
    pub confidence: Option<u8>,
    #[serde(default)]
    pub freshness: Option<u8>,
    #[serde(default)]
    pub retention: Option<Retention>,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lifecycle {
    pub importance: u8,
    pub sensitivity: Sensitivity,
    pub confidence: u8,
    pub freshness: u8,
    pub retention: Retention,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
}

impl Lifecycle {
    pub(crate) fn observation(
        input: &LifecycleInput,
        importance_hint: Option<&str>,
        content: &str,
        context: &WorkContext,
    ) -> Self {
        let retention = input.retention.clone().unwrap_or_default();
        Self {
            importance: input
                .importance
                .or_else(|| importance_hint.map(importance_score))
                .unwrap_or(50),
            sensitivity: floor_sensitivity(input.sensitivity.as_ref(), content, &[], context),
            confidence: input.confidence.unwrap_or(50),
            freshness: input.freshness.unwrap_or(100),
            retention: retention.clone(),
            expires_at: input.expires_at.clone().or_else(|| {
                (retention == Retention::Temporary)
                    .then(|| (now_seconds() + DEFAULT_TEMPORARY_RETENTION_SECONDS).to_string())
            }),
        }
    }

    pub(crate) fn durable_from(observations: &[Observation]) -> Self {
        let importance = observations
            .iter()
            .map(|observation| observation.lifecycle.importance)
            .max()
            .unwrap_or(50);
        let confidence = observations
            .iter()
            .map(|observation| observation.lifecycle.confidence as u16)
            .sum::<u16>()
            .checked_div(observations.len().max(1) as u16)
            .unwrap_or(50) as u8;
        let sensitivity = observations
            .iter()
            .map(|observation| observation.lifecycle.sensitivity.clone())
            .max_by_key(sensitivity_rank)
            .unwrap_or_default();
        Self {
            importance,
            sensitivity,
            confidence,
            freshness: 100,
            retention: Retention::Durable,
            expires_at: None,
        }
    }

    pub(crate) fn explicit(
        input: &LifecycleInput,
        content: &str,
        concepts: &[String],
        context: &WorkContext,
    ) -> Self {
        let retention = input.retention.clone().unwrap_or(Retention::Durable);
        Self {
            importance: input.importance.unwrap_or(80),
            sensitivity: floor_sensitivity(input.sensitivity.as_ref(), content, concepts, context),
            confidence: input.confidence.unwrap_or(80),
            freshness: input.freshness.unwrap_or(100),
            retention: retention.clone(),
            expires_at: input.expires_at.clone().or_else(|| {
                (retention == Retention::Temporary)
                    .then(|| (now_seconds() + DEFAULT_TEMPORARY_RETENTION_SECONDS).to_string())
            }),
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            importance: 50,
            sensitivity: Sensitivity::Private,
            confidence: 50,
            freshness: 100,
            retention: Retention::Temporary,
            expires_at: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleSummary {
    pub temporary_observations: usize,
    pub durable_memories: usize,
    pub sensitive_items: usize,
    pub expired_items: usize,
}

/// Adds a tombstone for each expired row that is not a protected source. Pure staging: the
/// caller persists.
pub(crate) fn sweep_expired_state(state: &mut StoreState) -> usize {
    let protected_sources = state
        .memories
        .iter()
        .filter(|memory| keeps_expired_source(memory))
        .filter(|memory| !state.forgotten_ids.contains(&memory.memory_id))
        .flat_map(|memory| memory.source_event_ids.clone())
        .collect::<HashSet<_>>();
    let mut expired_ids = Vec::new();
    for observation in &state.observations {
        if is_expired(&observation.lifecycle) && !protected_sources.contains(&observation.event_id)
        {
            expired_ids.push(observation.event_id.clone());
        }
    }
    for memory in &state.memories {
        if is_expired(&memory.lifecycle) {
            expired_ids.push(memory.memory_id.clone());
        }
    }
    expired_ids
        .into_iter()
        .filter(|id| state.forgotten_ids.insert(id.clone()))
        .count()
}

impl MemoryStore {
    pub fn sweep_expired(&mut self) -> Result<usize, StoreError> {
        let added = sweep_expired_state(&mut self.state);
        if added > 0 {
            self.persist()?;
        }
        Ok(added)
    }
}

impl MemoryStore {
    pub fn forget(&mut self, id: &str) -> Result<(), StoreError> {
        let sources = self.hide_id(id, None)?;
        self.note_sync_deletions(None, id, &sources);
        self.persist()
    }

    pub fn forget_for_account(&mut self, account_token: &str, id: &str) -> Result<(), StoreError> {
        self.hide_for_account(account_token, id, "memory_forget")
    }

    pub fn delete_for_account(&mut self, account_token: &str, id: &str) -> Result<(), StoreError> {
        self.hide_for_account(account_token, id, "memory_delete_retained")
    }

    fn hide_for_account(
        &mut self,
        account_token: &str,
        id: &str,
        action: &str,
    ) -> Result<(), StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.reject_member_mutation(&account, id)?;
        let person_id = account.person_id.clone();
        // Only an id this person owns. An unowned legacy id reads like an unknown one.
        let published = self.state.memories.iter().any(|m| {
            m.memory_id == id && (m.published_by.is_some() || m.scope == MemoryScope::Global)
        });
        if !published && self.item_owner(id).as_deref() != Some(person_id.as_str()) {
            return Err(StoreError::Validation(format!(
                "unknown memory or event: {id}"
            )));
        }
        if published
            && let Some(memory) = self.state.memories.iter_mut().find(|m| m.memory_id == id)
        {
            memory.changed_by = Some(person_id.clone());
            memory.changed_at = Some(crate::types::timestamp());
        }
        let sources = if published {
            // Hiding shared knowledge must not also hide its captures or sibling notes.
            self.state.forgotten_ids.insert(id.to_owned());
            Vec::new()
        } else {
            self.hide_id(id, Some(&person_id))?
        };
        self.note_sync_deletions(Some(&person_id), id, &sources);
        self.record_share_audit(&person_id, action, Some(id), serde_json::json!({}));
        self.persist()
    }

    fn source_ids(&self, id: &str) -> Vec<String> {
        self.state
            .memories
            .iter()
            .find(|memory| memory.memory_id == id)
            // A promoted copy never cascades into its origin's events.
            .filter(|memory| memory.promoted_from.is_none())
            .map(|memory| memory.source_event_ids.clone())
            .unwrap_or_default()
    }

    fn note_sync_deletions(&mut self, person_id: Option<&str>, item_id: &str, sources: &[String]) {
        let persons = match person_id {
            Some(person_id) => vec![person_id.to_owned()],
            None => {
                let mut persons = self.persons_for_item(item_id);
                for source in sources {
                    for person in self.persons_for_item(source) {
                        if !persons.contains(&person) {
                            persons.push(person);
                        }
                    }
                }
                persons
            }
        };
        for person in persons {
            self.push_sync_deletion(&person, item_id);
            for source in sources {
                self.push_sync_deletion(&person, source);
            }
        }
    }

    fn persons_for_item(&self, item_id: &str) -> Vec<String> {
        let mut persons = Vec::new();
        for event in &self.state.sync_events {
            if event.event_id == item_id && !persons.contains(&event.person_id) {
                persons.push(event.person_id.clone());
            }
        }
        if let Some(owner) = self.item_owner(item_id)
            && !persons.contains(&owner)
        {
            persons.push(owner);
        }
        persons
    }

    fn push_sync_deletion(&mut self, person_id: &str, item_id: &str) {
        if person_id.is_empty() || item_id.is_empty() {
            return;
        }
        if self
            .state
            .sync_deletions
            .iter()
            .any(|deletion| deletion.person_id == person_id && deletion.item_id == item_id)
        {
            return;
        }
        self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
        self.state.sync_deletions.push(SyncDeletionRecord {
            cursor: self.state.next_sync_cursor,
            person_id: person_id.to_owned(),
            item_id: item_id.to_owned(),
        });
    }

    /// Tombstones `id` and returns the source ids of that memory it tombstoned too. Trusted
    /// unscoped calls (`owner` is `None`) cascade every source id. An account cascades only
    /// the sources it owns: a source id in a row's metadata is never authority over another
    /// person's or a future id.
    fn hide_id(&mut self, id: &str, owner: Option<&str>) -> Result<Vec<String>, StoreError> {
        if id.trim().is_empty() {
            return Err(StoreError::Validation("id must not be empty".to_owned()));
        }
        let exists = self
            .state
            .observations
            .iter()
            .any(|item| item.event_id == id)
            || self.state.memories.iter().any(|item| item.memory_id == id);
        if !exists {
            return Err(StoreError::Validation(format!(
                "unknown memory or event: {id}"
            )));
        }
        self.state.forgotten_ids.insert(id.to_owned());
        let sources = self
            .source_ids(id)
            .into_iter()
            .filter(|source| {
                owner.is_none_or(|person_id| self.item_owner(source).as_deref() == Some(person_id))
            })
            .collect::<Vec<_>>();
        self.state.forgotten_ids.extend(sources.iter().cloned());
        Ok(sources)
    }

    pub fn lifecycle_summary(&self) -> LifecycleSummary {
        self.summarize_lifecycle(None)
    }

    pub fn lifecycle_summary_for_account(
        &self,
        account_token: &str,
    ) -> Result<LifecycleSummary, StoreError> {
        let account = self.authenticate_account(account_token)?;
        Ok(self.summarize_lifecycle(Some(account.person_id.as_str())))
    }

    /// Counts every row, or only rows the person owns.
    fn summarize_lifecycle(&self, person_id: Option<&str>) -> LifecycleSummary {
        let observations = self.state.observations.iter().filter(|observation| {
            person_id
                .is_none_or(|person_id| self.observation_owned_by(&observation.event_id, person_id))
        });
        let memories = self.state.memories.iter().filter(|memory| {
            person_id.is_none_or(|person_id| self.memory_owned_by(&memory.memory_id, person_id))
        });
        LifecycleSummary {
            temporary_observations: observations
                .clone()
                .filter(|observation| observation.lifecycle.retention == Retention::Temporary)
                .count(),
            durable_memories: memories
                .clone()
                .filter(|memory| memory.lifecycle.retention == Retention::Durable)
                .count(),
            sensitive_items: observations
                .clone()
                .filter(|observation| sensitivity_rank(&observation.lifecycle.sensitivity) >= 2)
                .count()
                + memories
                    .clone()
                    .filter(|memory| sensitivity_rank(&memory.lifecycle.sensitivity) >= 2)
                    .count(),
            expired_items: observations
                .filter(|observation| is_expired(&observation.lifecycle))
                .count()
                + memories
                    .filter(|memory| is_expired(&memory.lifecycle))
                    .count(),
        }
    }
}

pub(crate) fn validate_lifecycle_input(input: &LifecycleInput) -> Result<(), StoreError> {
    for (name, value) in [
        ("importance", input.importance),
        ("confidence", input.confidence),
        ("freshness", input.freshness),
    ] {
        if let Some(value) = value
            && value > 100
        {
            return Err(StoreError::Validation(format!(
                "lifecycle {name} must be between 0 and 100"
            )));
        }
    }
    if let Some(expires_at) = &input.expires_at {
        validate_timestamp(expires_at, "lifecycle expires_at")?;
    }
    Ok(())
}

pub(crate) fn validate_lifecycle(lifecycle: &Lifecycle) -> Result<(), StoreError> {
    for (name, value) in [
        ("importance", lifecycle.importance),
        ("confidence", lifecycle.confidence),
        ("freshness", lifecycle.freshness),
    ] {
        if value > 100 {
            return Err(StoreError::Validation(format!(
                "lifecycle {name} must be between 0 and 100"
            )));
        }
    }
    if let Some(expires_at) = &lifecycle.expires_at {
        validate_timestamp(expires_at, "lifecycle expires_at")?;
    }
    Ok(())
}

// PostgreSQL reads cast this string with `::bigint`, so the bound is i64::MAX, not u64::MAX.
fn validate_timestamp(value: &str, field: &str) -> Result<(), StoreError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|seconds| i64::try_from(*seconds).is_ok())
        .map(|_| ())
        .ok_or_else(|| {
            StoreError::Validation(format!(
                "{field} must be a Unix timestamp in seconds between 0 and {}",
                i64::MAX
            ))
        })
}

fn importance_score(value: &str) -> u8 {
    match value.trim().to_ascii_lowercase().as_str() {
        "critical" => 100,
        "high" => 80,
        "medium" => 50,
        "low" => 20,
        _ => 50,
    }
}

fn classify_sensitivity(content: &str) -> Sensitivity {
    let lower = content.to_ascii_lowercase();
    if ["authorization: bearer ", "api_key=", "token=", "password="]
        .iter()
        .any(|marker| lower.contains(marker))
        || find_token_prefix(&lower, "sk-", 0).is_some()
        || contains_private_key_header(content)
    {
        Sensitivity::Secret
    } else if ["private", "confidential", "internal-only", "personal data"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        Sensitivity::Sensitive
    } else {
        Sensitivity::Private
    }
}

fn floor_sensitivity(
    supplied: Option<&Sensitivity>,
    content: &str,
    concepts: &[String],
    context: &WorkContext,
) -> Sensitivity {
    let mut detected = classify_sensitivity(content);
    for text in concepts
        .iter()
        .map(String::as_str)
        .chain(human_context_texts(context))
    {
        if text.is_empty() {
            continue;
        }
        detected = stricter_sensitivity(detected, classify_sensitivity(text));
    }
    match supplied {
        Some(supplied) if sensitivity_rank(supplied) >= sensitivity_rank(&detected) => {
            supplied.clone()
        }
        _ => detected,
    }
}

pub(crate) fn stricter_sensitivity(current: Sensitivity, next: Sensitivity) -> Sensitivity {
    if sensitivity_rank(&current) >= sensitivity_rank(&next) {
        current
    } else {
        next
    }
}

/// Raises `current` when the human-readable context lists detect a stricter
/// label. Repository, branch, worktree, and commit are not scanned.
pub fn sensitivity_including_context(current: &Sensitivity, context: &WorkContext) -> Sensitivity {
    floor_sensitivity(Some(current), "", &[], context)
}

fn sensitivity_rank(value: &Sensitivity) -> u8 {
    match value {
        Sensitivity::Public => 0,
        Sensitivity::Private => 1,
        Sensitivity::Sensitive => 2,
        Sensitivity::Secret => 3,
    }
}

pub(crate) fn is_expired(lifecycle: &Lifecycle) -> bool {
    lifecycle
        .expires_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|expires_at| expires_at <= now_seconds())
}

#[cfg(test)]
mod tests;
