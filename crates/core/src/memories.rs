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
    pub created_at: String,
}

impl MemoryStore {
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
        let memory = self.stage_explicit_memory(content, concepts, context, lifecycle, kind)?;
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
        let memory = Memory {
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
        let memory = self.stage_correction(target_id, content, concepts, context, lifecycle)?;
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.record_generated_source_owners(&account.person_id, &memory);
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
        created_at: timestamp(),
    })
}

#[cfg(test)]
mod tests;
