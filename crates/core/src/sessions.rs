use crate::capture::Observation;
use crate::jobs::{
    BackgroundJob, JobEnd, JobKind, distill_job_end, distill_key, parse_distill_key,
};
use crate::lifecycle::{
    Lifecycle, MemoryKind, MemoryScope, Retention, is_expired, stricter_sensitivity,
};
use crate::memories::{MAX_MEMORY_BYTES, Memory};
use crate::privacy::{redact_memory_metadata, redact_work_context};
use crate::store::MemoryStore;
use crate::types::{StoreError, WorkContext, now_seconds, timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_HANDOFF_ITEMS: usize = 16;

pub(crate) const MAX_HANDOFF_ITEM_BYTES: usize = 2048;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    /// With `session_id`, the file session key. Absent on a legacy or unscoped row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
    pub context: WorkContext,
    pub started_at: String,
    pub ended_at: Option<String>,
}

/// An open session whose newest observation is older than this is finished on the next capture
/// or session start of the same person.
const IDLE_SESSION_SECS: u64 = 30 * 60;

/// At most this many idle sessions are finished per person per trigger; the next one continues.
const MAX_SWEEP_SESSIONS: usize = 16;

impl MemoryStore {
    pub fn start_session(
        &mut self,
        session_id: impl Into<String>,
        context: WorkContext,
    ) -> Result<Session, StoreError> {
        self.start_scoped_session(None, session_id, context)
    }

    pub fn start_session_for_account(
        &mut self,
        account_token: &str,
        session_id: impl Into<String>,
        context: WorkContext,
    ) -> Result<Session, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.start_scoped_session(Some(account.person_id), session_id, context)
    }

    fn start_scoped_session(
        &mut self,
        person_id: Option<String>,
        session_id: impl Into<String>,
        context: WorkContext,
    ) -> Result<Session, StoreError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }

        if let Some(person_id) = person_id.as_deref() {
            self.finish_idle_sessions(person_id);
        }
        if let Some(index) =
            self.state.sessions.iter().position(|session| {
                session.session_id == session_id && session.person_id == person_id
            })
        {
            // A push-created row is empty; the first real context fills it, as on PostgreSQL.
            let existing = &self.state.sessions[index];
            if existing.context == WorkContext::default() && context != WorkContext::default() {
                self.state.sessions[index].context = redact_work_context(&context);
                self.persist()?;
            }
            return Ok(self.state.sessions[index].clone());
        }

        // A session has no sensitivity field. Redact the human-readable lists
        // now. A later empty capture classifies that stored text, not the
        // original secret, so the observation label is not raised by a secret
        // that existed only here. The secret bytes are not stored.
        let context = redact_work_context(&context);
        let session = Session {
            session_id,
            person_id,
            context,
            started_at: timestamp(),
            ended_at: None,
        };
        self.state.sessions.push(session.clone());
        self.persist()?;
        Ok(session)
    }

    /// Finishes this person's idle sessions that still need a summary. A session that already
    /// has a distill job is left for that job. A quarantined session waits for a newer
    /// observation. Failures are logged; the trigger that called this has already succeeded.
    pub(crate) fn finish_idle_sessions(&mut self, person_id: &str) {
        let now = now_seconds();
        let retained = self.retained_by_session(person_id);
        let superseded = self.live_superseded_targets();
        let idle: Vec<String> = self
            .state
            .sessions
            .iter()
            .filter(|session| session.person_id.as_deref() == Some(person_id))
            .filter(|session| {
                let Some(observations) = retained.get(session.session_id.as_str()) else {
                    return false;
                };
                let newest = observations
                    .iter()
                    .map(|observation| observation.captured_at.parse::<u64>().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                now.saturating_sub(newest) > IDLE_SESSION_SECS
                    && self.session_needs_summary(person_id, session, observations, &superseded)
            })
            .map(|session| session.session_id.clone())
            .take(MAX_SWEEP_SESSIONS)
            .collect();
        drop((retained, superseded));
        self.finish_sessions(person_id, &idle);
    }

    /// Periodic close: the idle-session sweep for every person that has a session.
    pub fn sweep_idle_sessions(&mut self) {
        let people: HashSet<String> = self
            .state
            .sessions
            .iter()
            .filter_map(|session| session.person_id.clone())
            .collect();
        for person_id in people {
            self.finish_idle_sessions(&person_id);
        }
    }

    fn finish_sessions(&mut self, person_id: &str, session_ids: &[String]) {
        for session_id in session_ids {
            if self.distill_is_idle_paused(person_id, session_id) {
                continue;
            }
            if let Err(error) = self.finish_session(Some(person_id.to_owned()), session_id)
                && !matches!(distill_job_end(&error.to_string()), JobEnd::Succeeded)
            {
                eprintln!("cairn: automatic session close failed: {error}");
            }
        }
    }

    /// This person's retained observations grouped by session: owned, not forgotten, not
    /// expired. The same filter `finish_session_staged` summarises.
    fn retained_by_session(&self, person_id: &str) -> HashMap<&str, Vec<&Observation>> {
        let owned: HashSet<&str> = self
            .state
            .observation_owners
            .iter()
            .filter(|record| record.person_id == person_id)
            .map(|record| record.event_id.as_str())
            .collect();
        let mut grouped: HashMap<&str, Vec<&Observation>> = HashMap::new();
        for observation in &self.state.observations {
            if owned.contains(observation.event_id.as_str())
                && !self.state.forgotten_ids.contains(&observation.event_id)
                && !is_expired(&observation.lifecycle)
            {
                grouped
                    .entry(observation.session_id.as_str())
                    .or_default()
                    .push(observation);
            }
        }
        grouped
    }

    /// A summary the person hid is left alone: its id is forgotten, or a live correction
    /// supersedes it. Deriving it again under the same id would undo that action.
    fn session_needs_summary(
        &self,
        person_id: &str,
        session: &Session,
        observations: &[&Observation],
        superseded: &HashSet<&str>,
    ) -> bool {
        let memory_id = session_memory_id(Some(person_id), &session.session_id);
        let Some(summary) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
        else {
            return true;
        };
        !self.state.forgotten_ids.contains(&memory_id)
            && !superseded.contains(memory_id.as_str())
            && (session.ended_at.is_none()
                || summary_is_stale(summary, observations, &self.state.forgotten_ids))
    }

    pub fn end_session(&mut self, session_id: &str) -> Result<Memory, StoreError> {
        self.finish_session(None, session_id)
    }

    pub fn end_session_for_account(
        &mut self,
        account_token: &str,
        session_id: &str,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let person_id = account.person_id;
        self.finish_session(Some(person_id), session_id)
    }

    /// A refused end leaves the session open: `ended_at` is set before the checks that can fail.
    /// The distill job is claimed in the same write as its enqueue, then the summary runs.
    fn finish_session(
        &mut self,
        person_id: Option<String>,
        session_id: &str,
    ) -> Result<Memory, StoreError> {
        let now = now_seconds();
        let key = distill_key(person_id.as_deref(), session_id);
        let claim = self.with_jobs(|book| {
            book.enqueue(JobKind::Distill, &key, now, true);
            book.claim_key(JobKind::Distill, &key, now)
        });
        let Some(claim) = claim else {
            self.state = self.persisted.clone();
            return Err(StoreError::Validation(
                "distill job was not claimable".to_owned(),
            ));
        };
        self.persist()?;
        self.finish_claimed_distill(claim)
    }

    pub fn finish_claimed_distill(&mut self, claim: BackgroundJob) -> Result<Memory, StoreError> {
        let claim_id = claim.claim_id.clone().unwrap_or_default();
        let owns = self.state.jobs.iter().any(|job| {
            job.job_id == claim.job_id
                && job.claim_id.as_deref() == Some(claim_id.as_str())
                && !job.succeeded
        });
        if !owns {
            return Err(StoreError::Conflict(
                "distill claim is no longer held".to_owned(),
            ));
        }
        let Some((person_id, session_id)) = parse_distill_key(&claim.key) else {
            let now = now_seconds();
            let claim_id = claim.claim_id.unwrap_or_default();
            self.settle_job(
                &claim.job_id,
                &claim_id,
                now,
                JobEnd::Quarantine("distill job key is invalid".to_owned()),
            )?;
            return Err(StoreError::Validation(
                "distill job key is invalid".to_owned(),
            ));
        };
        let finished = self.finish_session_staged(person_id, &session_id);
        if finished.is_err() {
            self.state = self.persisted.clone();
        }
        let now = now_seconds();
        let claim_id = claim.claim_id.clone().unwrap_or_default();
        let end = match &finished {
            Ok(_) => JobEnd::Succeeded,
            Err(error) => distill_job_end(&error.to_string()),
        };
        if let Err(persist_error) = self.settle_job(&claim.job_id, &claim_id, now, end) {
            return match finished {
                Err(original) => Err(original),
                Ok(_) => Err(persist_error),
            };
        }
        finished
    }

    fn finish_session_staged(
        &mut self,
        person_id: Option<String>,
        session_id: &str,
    ) -> Result<Memory, StoreError> {
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }

        let memory_id = session_memory_id(person_id.as_deref(), session_id);
        let hidden = self.state.forgotten_ids.contains(&memory_id)
            || self.live_superseded_targets().contains(memory_id.as_str());
        let owned_events = person_id.as_ref().map(|person_id| {
            self.state
                .observation_owners
                .iter()
                .filter(|record| record.person_id == *person_id)
                .map(|record| record.event_id.clone())
                .collect::<HashSet<_>>()
        });
        let session = self
            .state
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id && session.person_id == person_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown session: {session_id}")))?;
        if session.ended_at.is_none() {
            session.ended_at = Some(timestamp());
        }

        let observations: Vec<Observation> = self
            .state
            .observations
            .iter()
            .filter(|observation| {
                let in_scope = match owned_events.as_ref() {
                    Some(events) => events.contains(&observation.event_id),
                    None => !self
                        .state
                        .observation_owners
                        .iter()
                        .any(|record| record.event_id == observation.event_id),
                };
                observation.session_id == session_id
                    && !self.state.forgotten_ids.contains(&observation.event_id)
                    && !is_expired(&observation.lifecycle)
                    && in_scope
            })
            .cloned()
            .collect();

        if observations.is_empty() {
            return Err(StoreError::Validation(format!(
                "session has no retained observations: {session_id}"
            )));
        }

        let mut earlier = None;
        if let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
        {
            if hidden {
                // Forgotten or corrected by the person: respect it, do not derive it again.
                return Err(StoreError::Validation(format!(
                    "session summary was forgotten or superseded: {memory_id}"
                )));
            }
            let refs: Vec<&Observation> = observations.iter().collect();
            if !summary_is_stale(&memory, &refs, &self.state.forgotten_ids) {
                self.persist()?;
                return Ok(memory);
            }
            // The session id was reused after its summary, or a source was forgotten:
            // derive the summary again from what is retained.
            self.state
                .memories
                .retain(|existing| existing.memory_id != memory_id);
            earlier = Some(memory);
        }

        let mut memory = derive_session_memory(
            person_id.as_deref(),
            session_id,
            &session.context,
            &observations,
        )?;
        if memory.project_id.as_deref().is_some_and(|id| {
            person_id.as_deref().is_none_or(|person| {
                self.project_owner_id(id).as_deref() != Some(person)
                    && !self
                        .state
                        .project_members
                        .iter()
                        .any(|m| m.project_id == id && m.member_person_id == person)
            })
        }) {
            memory.scope = MemoryScope::Personal;
            memory.project_id = None;
            memory.published_by = None;
        }
        if let Some(earlier) = &earlier {
            carry_summary_state(earlier, &mut memory);
        }
        let superseded = self.live_superseded_targets();
        let mut notes = crate::distill_session_notes(&memory, &observations);
        notes.retain(|note| {
            !self.state.forgotten_ids.contains(&note.memory_id)
                && !superseded.contains(note.memory_id.as_str())
        });
        for mut note in notes {
            if let Some(person_id) = person_id.as_deref() {
                self.record_memory_owner(person_id, &note.memory_id);
            }
            if let Some(existing) = self
                .state
                .memories
                .iter_mut()
                .find(|m| m.memory_id == note.memory_id)
            {
                carry_summary_state(existing, &mut note);
                *existing = note;
            } else {
                self.state.memories.push(note);
            }
        }
        self.state.memories.push(memory.clone());
        if let Some(person_id) = person_id.as_deref() {
            self.record_memory_owner(person_id, &memory.memory_id);
        }
        self.persist()?;
        Ok(memory)
    }
}

struct SessionDigest {
    content: String,
    next_steps: Vec<String>,
    unresolved_work: Vec<String>,
}

fn build_session_digest(observations: &[Observation]) -> SessionDigest {
    let mut content = String::from("Session summary (deterministically compressed):\n");
    let mut seen = HashSet::new();
    let mut omitted = 0;
    let mut next_steps = Vec::new();
    let mut unresolved_work = Vec::new();
    let mut source_actors = Vec::new();
    for observation in observations {
        if !observation.source.trim().is_empty() {
            push_unique(&mut source_actors, observation.source.clone());
        }
        let normalized = observation
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !seen.insert(normalized.clone()) {
            omitted += 1;
            continue;
        }
        let line = format!("- {} [evidence: {}]\n", normalized, observation.event_id);
        if content.len() + line.len() > MAX_MEMORY_BYTES {
            omitted += 1;
            continue;
        }
        content.push_str("- ");
        content.push_str(&normalized);
        content.push_str(" [evidence: ");
        content.push_str(&observation.event_id);
        content.push_str("]\n");

        for source_line in observation.content.lines() {
            let normalized = source_line.split_whitespace().collect::<Vec<_>>().join(" ");
            let lower = normalized.to_ascii_lowercase();
            let evidence = format!("{normalized} [evidence: {}]", observation.event_id);
            if contains_marker(
                &lower,
                &[
                    "next step",
                    "next:",
                    "follow up",
                    "follow-up",
                    "should ",
                    "need to ",
                    "todo",
                    "remaining",
                ],
            ) {
                push_unique(&mut next_steps, evidence.clone());
            }
            if contains_marker(
                &lower,
                &[
                    "todo",
                    "unresolved",
                    "blocked",
                    "not yet",
                    "not covered",
                    "untested",
                    "follow up",
                    "follow-up",
                    "remaining",
                ],
            ) {
                push_unique(&mut unresolved_work, evidence);
            }
        }
    }
    if omitted > 0 {
        content.push_str(&format!("- {omitted} repeated or oversized observations omitted; source IDs remain attached to the memory.\n"));
    }
    append_digest_section(&mut content, "Next steps", &next_steps);
    append_digest_section(&mut content, "Unresolved work", &unresolved_work);
    append_digest_section(&mut content, "Source actors", &source_actors);
    SessionDigest {
        content,
        next_steps,
        unresolved_work,
    }
}

/// A summary is stale when a retained observation is not among its sources, or a source
/// has been forgotten (the summary is hidden until it is derived again without it).
fn summary_is_stale(
    summary: &Memory,
    observations: &[&Observation],
    forgotten_ids: &HashSet<String>,
) -> bool {
    observations
        .iter()
        .any(|observation| !summary.source_event_ids.contains(&observation.event_id))
        || summary
            .source_event_ids
            .iter()
            .any(|event_id| forgotten_ids.contains(event_id))
}

/// A re-derived summary keeps verification, lifecycle (sensitivity can only rise),
/// creation time, reinforcement, feedback, and its declared links.
pub fn carry_summary_state(earlier: &Memory, derived: &mut Memory) {
    derived.lifecycle = Lifecycle {
        sensitivity: stricter_sensitivity(
            earlier.lifecycle.sensitivity.clone(),
            derived.lifecycle.sensitivity.clone(),
        ),
        ..earlier.lifecycle.clone()
    };
    derived.verified_commit.clone_from(&earlier.verified_commit);
    derived.created_at.clone_from(&earlier.created_at);
    derived.access_count = earlier.access_count;
    derived
        .last_accessed_at
        .clone_from(&earlier.last_accessed_at);
    derived.feedback.clone_from(&earlier.feedback);
    derived.contradicts.clone_from(&earlier.contradicts);
    derived.supersedes.clone_from(&earlier.supersedes);
    derived.helpful_count = earlier.helpful_count;
    derived.stale_count = earlier.stale_count;
}

pub fn session_memory_id(person_id: Option<&str>, session_id: &str) -> String {
    match person_id.filter(|person_id| !person_id.is_empty()) {
        Some(person_id) => format!("memory-{person_id}-{session_id}"),
        None => format!("memory-{session_id}"),
    }
}

pub(crate) fn keeps_expired_source(memory: &Memory) -> bool {
    if memory.lifecycle.retention != Retention::Durable || is_expired(&memory.lifecycle) {
        return false;
    }
    true
}

pub fn derive_session_memory(
    person_id: Option<&str>,
    session_id: &str,
    session_context: &WorkContext,
    observations: &[Observation],
) -> Result<Memory, StoreError> {
    if session_id.trim().is_empty() {
        return Err(StoreError::Validation(
            "session_id must not be empty".to_owned(),
        ));
    }
    if observations.is_empty() {
        return Err(StoreError::Validation(
            "session has no retained observations".to_owned(),
        ));
    }
    let digest = build_session_digest(observations);
    let mut memory = Memory {
        fields: crate::MemoryFields {
            title: format!("Session {session_id}"),
            summary: format!(
                "Session {session_id}: {} retained captures.",
                observations.len()
            ),
            details: Some(digest.content.clone()),
            evidence: observations.iter().map(|o| o.event_id.clone()).collect(),
            tags: vec!["work-note".to_owned(), "distilled".to_owned()],
            author: person_id.map(str::to_owned),
            ..Default::default()
        },
        memory_id: session_memory_id(person_id, session_id),
        session_id: session_id.to_owned(),
        content: digest.content,
        source_event_ids: observations
            .iter()
            .map(|observation| observation.event_id.clone())
            .collect(),
        context: merge_context(observations),
        lifecycle: Lifecycle::durable_from(observations),
        kind: MemoryKind::SessionSummary,
        scope: session_scope(session_context),
        project_id: session_context.project_id.clone(),
        next_steps: digest.next_steps,
        unresolved_work: digest.unresolved_work,
        supersedes: None,
        contradicts: Vec::new(),
        promoted_from: None,
        promotion_reason: None,
        verified_commit: None,
        published_by: session_context
            .project_id
            .as_ref()
            .and(person_id)
            .map(str::to_owned),
        changed_by: None,
        changed_at: None,
        created_at: timestamp(),
        access_count: 0,
        last_accessed_at: None,
        helpful_count: 0,
        stale_count: 0,
        feedback: Vec::new(),
    };
    memory.project_id = memory.context.project_id.clone().filter(|id| {
        observations
            .iter()
            .all(|o| o.scope == MemoryScope::Project && o.context.project_id.as_ref() == Some(id))
    });
    memory.scope = if memory.project_id.is_some() {
        MemoryScope::Project
    } else {
        MemoryScope::Personal
    };
    memory.published_by = memory.project_id.as_ref().and(person_id).map(str::to_owned);
    memory.fields.importance = Some(memory.lifecycle.importance);
    memory.fields.applies_to.clone_from(&memory.context.files);
    if let Some(branch) = &memory.context.branch {
        push_unique(&mut memory.fields.applies_to, branch.clone());
    }
    memory.changed_by = person_id.map(str::to_owned);
    memory.changed_at = Some(memory.created_at.clone());
    redact_memory_metadata(&mut memory);
    Ok(memory)
}

fn append_digest_section(content: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    content.push('\n');
    content.push_str(title);
    content.push_str(":\n");
    for item in items {
        if content.len() + item.len() + 2 > MAX_MEMORY_BYTES {
            break;
        }
        content.push_str("- ");
        content.push_str(item);
        content.push('\n');
    }
}

fn contains_marker(value: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}

fn push_unique(items: &mut Vec<String>, item: String) {
    if items.len() >= MAX_HANDOFF_ITEMS || items.contains(&item) {
        return;
    }
    if item.len() <= MAX_HANDOFF_ITEM_BYTES {
        items.push(item);
    } else {
        let truncated = item
            .chars()
            .take(MAX_HANDOFF_ITEM_BYTES)
            .collect::<String>();
        items.push(truncated);
    }
}

fn merge_context(observations: &[Observation]) -> WorkContext {
    let mut context = WorkContext::default();
    for observation in observations {
        let source = &observation.context;
        context.project_id = context
            .project_id
            .clone()
            .or_else(|| source.project_id.clone());
        context.repository = context
            .repository
            .clone()
            .or_else(|| source.repository.clone());
        context.branch = context.branch.clone().or_else(|| source.branch.clone());
        context.worktree = context.worktree.clone().or_else(|| source.worktree.clone());
        context.commit = context.commit.clone().or_else(|| source.commit.clone());
        for file in &source.files {
            if !context.files.contains(file) {
                context.files.push(file.clone());
            }
        }
        merge_context_items(&mut context.symbols, &source.symbols);
        merge_context_items(&mut context.commands, &source.commands);
        merge_context_items(&mut context.tests, &source.tests);
        merge_context_items(&mut context.services, &source.services);
    }
    context
}

fn merge_context_items(target: &mut Vec<String>, source: &[String]) {
    for item in source {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}

fn session_scope(context: &WorkContext) -> MemoryScope {
    if context.project_id.is_some() {
        MemoryScope::Project
    } else {
        MemoryScope::Personal
    }
}

pub(crate) fn memory_scope(context: &WorkContext) -> MemoryScope {
    session_scope(context)
}

#[cfg(test)]
mod tests;
