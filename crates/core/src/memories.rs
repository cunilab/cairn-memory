use crate::capture::MAX_OBSERVATION_BYTES;
use crate::lifecycle::{
    Lifecycle, LifecycleInput, MemoryKind, MemoryScope, validate_lifecycle_input,
};
use crate::privacy::{redact_sensitive_content, redact_work_context};
use crate::sessions::memory_scope;
use crate::store::MemoryStore;
use crate::types::{StoreError, WorkContext, timestamp, timestamp_nanos};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_MEMORY_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Memory {
    #[serde(flatten)]
    pub fields: MemoryFields,
    pub memory_id: String,
    pub session_id: String,
    pub content: String,
    pub source_event_ids: Vec<String>,
    pub context: WorkContext,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub kind: MemoryKind,
    #[serde(default)]
    pub scope: MemoryScope,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub unresolved_work: Vec<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub promoted_from: Option<String>,
    #[serde(default)]
    pub promotion_reason: Option<String>,
    #[serde(default)]
    pub verified_commit: Option<String>,
    /// Set only by the publication transition; project tags alone never publish a row.
    #[serde(default)]
    pub published_by: Option<String>,
    #[serde(default)]
    pub changed_by: Option<String>,
    #[serde(default)]
    pub changed_at: Option<String>,
    pub created_at: String,
}

/// Common and typed note fields, flattened in the public memory record.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MemoryFields {
    pub title: String,
    pub summary: String,
    pub details: Option<String>,
    pub reason: String,
    pub applies_to: Vec<String>,
    pub evidence: Vec<String>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub status: MemoryStatus,
    pub pinned: bool,
    pub importance: Option<u8>,
    pub author: Option<String>,
    pub decision_context: Option<String>,
    pub alternatives: Vec<String>,
    pub consequences: Option<String>,
    pub rule: Option<String>,
    pub exceptions: Option<String>,
    pub symptom: Option<String>,
    pub cause: Option<String>,
    pub fix: Option<String>,
    pub steps: Vec<String>,
    pub when: Option<String>,
    pub statement: Option<String>,
    pub source: Option<String>,
    pub preference: Option<String>,
    pub example: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    #[default]
    Active,
    Stale,
    Superseded,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SaveMemoryInput {
    #[serde(flatten)]
    pub fields: MemoryFields,
    pub scope: Option<MemoryScope>,
    #[serde(default)]
    pub kind: MemoryKind,
    #[serde(default)]
    pub context: WorkContext,
    #[serde(default)]
    pub lifecycle: LifecycleInput,
    pub supersedes: Option<String>,
}

/// Shared validation and redaction for the two storage adapters.
pub fn build_saved_memory(mut input: SaveMemoryInput, author: &str) -> Result<Memory, StoreError> {
    if let Some(importance) = input.fields.importance {
        input.lifecycle.importance = Some(importance);
    }
    let scope = input.scope.ok_or_else(|| {
        StoreError::Validation("scope is required (project, personal, global)".to_owned())
    })?;
    if !matches!(
        scope,
        MemoryScope::Project | MemoryScope::Personal | MemoryScope::Global
    ) {
        return Err(StoreError::Validation(
            "scope must be project, personal or global".to_owned(),
        ));
    }
    for (name, value) in [
        ("title", &input.fields.title),
        ("summary", &input.fields.summary),
        ("reason", &input.fields.reason),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::Validation(format!("{name} is required")));
        }
    }
    if input.kind == MemoryKind::Preference && scope != MemoryScope::Personal {
        return Err(StoreError::Validation(
            "preference requires personal scope".to_owned(),
        ));
    }
    if scope == MemoryScope::Project && input.context.project_id.is_none() {
        return Err(StoreError::Validation(
            "project scope requires context.project_id".to_owned(),
        ));
    }
    let raw = serde_json::to_value(&input.fields)?;
    let text = serde_json::to_string(&raw)?;
    if text.len() > MAX_OBSERVATION_BYTES {
        return Err(StoreError::Validation(
            "memory fields exceed the 65536 byte limit".to_owned(),
        ));
    }
    let mut fields = raw;
    redact_field_values(&mut fields);
    let mut fields: MemoryFields = serde_json::from_value(fields)?;
    fields.author = Some(author.to_owned());
    let body = format!(
        "{}\n{}\n{}\n{}",
        fields.title,
        fields.summary,
        fields.details.as_deref().unwrap_or_default(),
        fields.reason
    );
    let context = input.context;
    let mut memory = build_explicit_memory_record(
        &body,
        fields.tags.clone(),
        context,
        input.lifecycle,
        input.kind,
    )?;
    // Inspect every supplied text field before redaction, including typed evidence.
    memory.lifecycle.sensitivity = Lifecycle::explicit(
        &LifecycleInput {
            sensitivity: Some(memory.lifecycle.sensitivity.clone()),
            ..LifecycleInput::default()
        },
        &text,
        &[],
        &memory.context,
    )
    .sensitivity;
    memory.fields = fields;
    memory.fields.importance = Some(memory.lifecycle.importance);
    memory.scope = scope;
    memory.supersedes = input.supersedes;
    if memory.scope == MemoryScope::Project {
        memory.project_id = memory.context.project_id.clone();
        memory.published_by = Some(author.to_owned());
    }
    memory.changed_by = Some(author.to_owned());
    memory.changed_at = Some(memory.created_at.clone());
    Ok(memory)
}

fn redact_field_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = redact_sensitive_content(text),
        serde_json::Value::Array(values) => values.iter_mut().for_each(redact_field_values),
        serde_json::Value::Object(values) => values.values_mut().for_each(redact_field_values),
        _ => {}
    }
}

/// A raw capture is a readable project record without a publication step.
pub fn capture_memory(observation: &crate::Observation, author: &str) -> Memory {
    Memory {
        fields: MemoryFields {
            title: observation
                .content
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect(),
            summary: observation.content.clone(),
            author: Some(author.to_owned()),
            importance: Some(observation.lifecycle.importance),
            evidence: vec![observation.event_id.clone()],
            tags: vec!["work-note".to_owned()],
            ..MemoryFields::default()
        },
        memory_id: format!("capture-{author}-{}", observation.event_id),
        session_id: observation.session_id.clone(),
        content: observation.content.clone(),
        source_event_ids: vec![observation.event_id.clone()],
        context: observation.context.clone(),
        lifecycle: observation.lifecycle.clone(),
        kind: MemoryKind::Capture,
        scope: MemoryScope::Project,
        project_id: observation.context.project_id.clone(),
        next_steps: Vec::new(),
        unresolved_work: Vec::new(),
        supersedes: None,
        promoted_from: None,
        promotion_reason: None,
        verified_commit: None,
        published_by: Some(author.to_owned()),
        changed_by: Some(author.to_owned()),
        changed_at: Some(observation.captured_at.clone()),
        created_at: observation.captured_at.clone(),
    }
}

impl MemoryStore {
    pub fn save(&mut self, input: SaveMemoryInput) -> Result<Memory, StoreError> {
        if self.has_accounts() {
            return Err(StoreError::Validation(
                "account authentication required".to_owned(),
            ));
        }
        let mut memory = build_saved_memory(input, "local")?;
        if memory.scope != MemoryScope::Personal {
            return Err(StoreError::Validation(
                "account authentication required for shared scopes".to_owned(),
            ));
        }
        memory.fields.author = None;
        self.state.memories.push(memory.clone());
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }

    pub fn save_for_account(
        &mut self,
        token: &str,
        input: SaveMemoryInput,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(token)?;
        let memory = build_saved_memory(input, &account.person_id)?;
        if let Some(id) = memory.project_id.as_deref() {
            self.require_project_reader(&account, id)?;
        }
        if let Some(id) = memory.supersedes.as_deref() {
            self.reject_member_mutation(&account, id)?;
            let target = self
                .state
                .memories
                .iter()
                .find(|m| m.memory_id == id)
                .ok_or_else(|| StoreError::Validation("unknown supersedes memory".to_owned()))?;
            if target.scope != memory.scope || target.project_id != memory.project_id {
                return Err(StoreError::Validation(
                    "supersedes must stay in the same scope and project".to_owned(),
                ));
            }
        }
        let entities_before = self.state.project_entities.len();
        self.state.memories.push(memory.clone());
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.record_generated_source_owners(&account.person_id, &memory);
        if let Some(id) = memory.project_id.as_deref() {
            self.index_project_memory(id, &memory);
        }
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            self.state.project_entities.truncate(entities_before);
            return Err(error);
        }
        Ok(memory)
    }

    pub fn remember(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
    ) -> Result<Memory, StoreError> {
        self.remember_with_lifecycle(content, concepts, context, LifecycleInput::default())
    }

    pub fn remember_with_lifecycle(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        self.remember_with_kind(content, concepts, context, lifecycle, MemoryKind::Explicit)
    }

    pub fn remember_lesson(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        self.remember_with_kind(content, concepts, context, lifecycle, MemoryKind::Lesson)
    }

    pub fn remember_with_kind(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, StoreError> {
        if content.trim().is_empty() {
            return Err(StoreError::Validation(
                "content must not be empty".to_owned(),
            ));
        }
        if content.len() > MAX_OBSERVATION_BYTES {
            return Err(StoreError::Validation(format!(
                "content exceeds the {} byte limit",
                MAX_OBSERVATION_BYTES
            )));
        }
        validate_lifecycle_input(&lifecycle)?;

        let memory = self.stage_explicit_memory(content, concepts, context, lifecycle, kind)?;
        self.persist()?;
        Ok(memory)
    }

    /// The validation and row of `remember_with_kind`, staged without persisting.
    fn stage_explicit_memory(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, StoreError> {
        let memory = build_explicit_memory_record(content, concepts, context, lifecycle, kind)?;
        self.state.memories.push(memory.clone());
        Ok(memory)
    }

    /// Own the source id this store just generated for `memory`, so a forget can cascade it
    /// and no other account can take it. Only generated ids: a source id named by imported
    /// metadata is never recorded.
    fn record_generated_source_owners(&mut self, person_id: &str, memory: &Memory) {
        for source in &memory.source_event_ids {
            self.record_observation_owner(person_id, source);
        }
    }

    pub fn remember_for_account(
        &mut self,
        account_token: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        // The row and its owner records land in one rename; a failed rename leaves none.
        let mut memory = self.stage_explicit_memory(content, concepts, context, lifecycle, kind)?;
        memory.fields.author = Some(account.person_id.clone());
        *self
            .state
            .memories
            .last_mut()
            .expect("staged explicit memory") = memory.clone();
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.record_generated_source_owners(&account.person_id, &memory);
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }

    /// Remove a staged generated memory with its owner records, so a failed rename leaves none.
    fn unstage_memory(&mut self, memory: &Memory) {
        self.state
            .memories
            .retain(|stored| stored.memory_id != memory.memory_id);
        self.state
            .memory_owners
            .retain(|record| record.memory_id != memory.memory_id);
        self.state
            .observation_owners
            .retain(|record| !memory.source_event_ids.contains(&record.event_id));
    }

    pub fn correct(
        &mut self,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        let memory = self.stage_correction(target_id, content, concepts, context, lifecycle)?;
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }

    /// The validation and row of `correct`, staged without persisting.
    fn stage_correction(
        &mut self,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        if target_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "target_id must not be empty".to_owned(),
            ));
        }
        if content.trim().is_empty() {
            return Err(StoreError::Validation(
                "content must not be empty".to_owned(),
            ));
        }
        if content.len() > MAX_OBSERVATION_BYTES {
            return Err(StoreError::Validation(format!(
                "content exceeds the {} byte limit",
                MAX_OBSERVATION_BYTES
            )));
        }
        validate_lifecycle_input(&lifecycle)?;
        let target = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == target_id)
            .map(|memory| {
                (
                    memory.memory_id.clone(),
                    memory.context.clone(),
                    memory.scope.clone(),
                    memory.project_id.clone(),
                )
            })
            .or_else(|| {
                self.state
                    .observations
                    .iter()
                    .find(|observation| observation.event_id == target_id)
                    .map(|observation| {
                        (
                            observation.event_id.clone(),
                            observation.context.clone(),
                            memory_scope(&observation.context),
                            None,
                        )
                    })
            })
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown memory or event: {target_id}"))
            })?;

        let suffix = timestamp_nanos();
        let correction_event = format!("event-correction-{suffix}");
        let inherited = context == WorkContext::default();
        let source_context = if inherited {
            target.1.clone()
        } else {
            context.clone()
        };
        let mut lifecycle = Lifecycle::explicit(&lifecycle, content, &concepts, &source_context);
        lifecycle.confidence = 100;
        let mut fields = self
            .state
            .memories
            .iter()
            .find(|m| m.memory_id == target_id)
            .map(|m| m.fields.clone())
            .unwrap_or_default();
        if fields.title.is_empty() || fields.reason.trim().is_empty() {
            fields.title = redact_sensitive_content(content)
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect();
        }
        fields.summary = redact_sensitive_content(content);
        fields.details = Some(redact_sensitive_content(content));
        let memory = Memory {
            fields,
            memory_id: format!("memory-correction-{suffix}"),
            session_id: format!("session-correction-{suffix}"),
            content: build_explicit_memory(content, concepts),
            source_event_ids: vec![correction_event],
            context: redact_work_context(&source_context),
            lifecycle,
            kind: MemoryKind::Correction,
            scope: if inherited {
                target.2
            } else {
                memory_scope(&context)
            },
            project_id: target.3,
            next_steps: Vec::new(),
            unresolved_work: Vec::new(),
            supersedes: Some(target.0),
            promoted_from: None,
            promotion_reason: None,
            verified_commit: None,
            published_by: None,
            changed_by: None,
            changed_at: None,
            created_at: timestamp(),
        };
        self.state.memories.push(memory.clone());
        Ok(memory)
    }

    pub fn correct_for_account(
        &mut self,
        account_token: &str,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.reject_member_mutation(&account, target_id)?;
        // The row and its owner records land in one rename; a failed rename leaves none.
        let publication = self
            .state
            .memories
            .iter()
            .find(|m| m.memory_id == target_id)
            .cloned();
        let mut memory = self.stage_correction(target_id, content, concepts, context, lifecycle)?;
        if let Some(target) = publication {
            memory.fields = target.fields;
            if memory.fields.reason.trim().is_empty() {
                memory.fields.title = redact_sensitive_content(content)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect();
            }
            memory.fields.summary = redact_sensitive_content(content);
            memory.fields.details = Some(redact_sensitive_content(content));
            memory.scope = target.scope;
            memory.context = target.context;
            memory.project_id = target.project_id;
            memory.published_by = target.published_by;
            memory.changed_by = Some(account.person_id.clone());
            memory.changed_at = Some(timestamp());
            if memory.scope == MemoryScope::Project {
                memory.source_event_ids.clear();
            }
            *self.state.memories.last_mut().expect("staged correction") = memory.clone();
        }
        // A project correction keeps its author's storage identity after ownership transfer.
        let owner_id = memory
            .fields
            .author
            .as_deref()
            .or(memory.published_by.as_deref())
            .unwrap_or(&account.person_id);
        self.record_memory_owner(owner_id, &memory.memory_id);
        self.record_generated_source_owners(owner_id, &memory);
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }
}

fn build_explicit_memory(content: &str, concepts: Vec<String>) -> String {
    let mut result = redact_sensitive_content(content);
    let concepts: Vec<String> = concepts
        .into_iter()
        .filter(|concept| !concept.trim().is_empty())
        .map(|concept| redact_sensitive_content(&concept))
        .collect();
    if !concepts.is_empty() {
        result.push_str("\nConcepts: ");
        result.push_str(&concepts.join(", "));
    }
    result
}

pub fn build_explicit_memory_record(
    content: &str,
    concepts: Vec<String>,
    context: WorkContext,
    lifecycle: LifecycleInput,
    kind: MemoryKind,
) -> Result<Memory, StoreError> {
    if content.trim().is_empty() {
        return Err(StoreError::Validation(
            "content must not be empty".to_owned(),
        ));
    }
    if content.len() > MAX_OBSERVATION_BYTES {
        return Err(StoreError::Validation(format!(
            "content exceeds the {} byte limit",
            MAX_OBSERVATION_BYTES
        )));
    }
    validate_lifecycle_input(&lifecycle)?;
    let suffix = timestamp_nanos();
    let event_id = format!("event-explicit-{suffix}");
    let record_lifecycle = Lifecycle::explicit(&lifecycle, content, &concepts, &context);
    let stored_context = redact_work_context(&context);
    Ok(Memory {
        fields: MemoryFields {
            title: redact_sensitive_content(content)
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect(),
            summary: redact_sensitive_content(content),
            importance: Some(record_lifecycle.importance),
            details: None,
            ..MemoryFields::default()
        },
        memory_id: format!("memory-explicit-{suffix}"),
        session_id: format!("session-explicit-{suffix}"),
        content: build_explicit_memory(content, concepts),
        source_event_ids: vec![event_id],
        context: stored_context,
        lifecycle: record_lifecycle,
        kind,
        scope: memory_scope(&context),
        project_id: None,
        next_steps: Vec::new(),
        unresolved_work: Vec::new(),
        supersedes: None,
        promoted_from: None,
        promotion_reason: None,
        verified_commit: None,
        published_by: None,
        changed_by: None,
        changed_at: None,
        created_at: timestamp(),
    })
}

#[cfg(test)]
mod tests;
