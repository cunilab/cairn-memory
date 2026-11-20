use crate::legacy::memories::Memory;
use crate::legacy::types::StoreError;
use crate::legacy::{MemoryKind, now_seconds};
use serde::{Deserialize, Serialize};

pub const SALIENCE_WEIGHT: f32 = 0.35;
pub const AGE_WEIGHT: f32 = 0.25;
pub const ACCESS_WEIGHT: f32 = 0.20;
pub const RECENCY_WEIGHT: f32 = 0.20;
pub const AGE_HALF_LIFE_DAYS: f32 = 30.0;
pub const RECENCY_HALF_LIFE_DAYS: f32 = 14.0;
pub const ACCESS_SATURATION: f32 = 5.0;
pub const RETENTION_THRESHOLD: f32 = 0.45;
pub const BELOW_THRESHOLD_FLOOR: f32 = 0.25;
pub const FEEDBACK_STEP: f32 = 0.15;
pub const FEEDBACK_CAP: f32 = 0.45;
const DAY_SECONDS: f32 = 86_400.0;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackVote {
    Helpful,
    Stale,
}

impl FeedbackVote {
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        match value.trim() {
            "helpful" => Ok(Self::Helpful),
            "stale" => Ok(Self::Stale),
            _ => Err(StoreError::Validation(
                "vote must be helpful or stale".to_owned(),
            )),
        }
    }
}

/// One person's current vote. Repeating a vote replaces it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFeedback {
    pub person_id: String,
    pub vote: FeedbackVote,
}

/// Captures, automatic session summaries, and handoffs decay. A purposeful save does not.
pub fn decays(kind: MemoryKind) -> bool {
    matches!(
        kind,
        MemoryKind::Capture | MemoryKind::SessionSummary | MemoryKind::Handoff
    )
}

pub fn retention_score(memory: &Memory, now_secs: u64) -> f32 {
    let salience = f32::from(memory.lifecycle.importance) / 100.0;
    let age = half_life(
        elapsed_days(&memory.created_at, now_secs),
        AGE_HALF_LIFE_DAYS,
    );
    let accessed = memory
        .last_accessed_at
        .as_deref()
        .unwrap_or(memory.created_at.as_str());
    let recency = half_life(elapsed_days(accessed, now_secs), RECENCY_HALF_LIFE_DAYS);
    let access = 1.0 - half_life(memory.access_count as f32, ACCESS_SATURATION);
    (SALIENCE_WEIGHT * salience
        + AGE_WEIGHT * age
        + ACCESS_WEIGHT * access
        + RECENCY_WEIGHT * recency)
        .clamp(0.0, 1.0)
}

/// 1 when the memory does not decay or its score is at least the threshold.
/// Below the threshold the factor stays in `[BELOW_THRESHOLD_FLOOR, 1)`.
pub fn retention_factor(memory: &Memory, now_secs: u64) -> f32 {
    if !decays(memory.kind) {
        return 1.0;
    }
    let score = retention_score(memory, now_secs);
    if score >= RETENTION_THRESHOLD {
        1.0
    } else {
        BELOW_THRESHOLD_FLOOR + (1.0 - BELOW_THRESHOLD_FLOOR) * (score / RETENTION_THRESHOLD)
    }
}

pub fn feedback_factor(helpful: u32, stale: u32) -> f32 {
    let net = helpful as f32 - stale as f32;
    (1.0 + FEEDBACK_STEP * net).clamp(1.0 - FEEDBACK_CAP, 1.0 + FEEDBACK_CAP)
}

pub fn ranking_multiplier(memory: &Memory, now_secs: u64) -> f32 {
    retention_factor(memory, now_secs) * feedback_factor(memory.helpful_count, memory.stale_count)
}

pub fn apply_feedback(memory: &mut Memory, person_id: &str, vote: FeedbackVote) {
    if let Some(existing) = memory
        .feedback
        .iter_mut()
        .find(|row| row.person_id == person_id)
    {
        existing.vote = vote;
    } else {
        memory.feedback.push(MemoryFeedback {
            person_id: person_id.to_owned(),
            vote,
        });
    }
    sync_feedback_counts(memory);
}

pub fn sync_feedback_counts(memory: &mut Memory) {
    memory.helpful_count = memory
        .feedback
        .iter()
        .filter(|row| row.vote == FeedbackVote::Helpful)
        .count() as u32;
    memory.stale_count = memory
        .feedback
        .iter()
        .filter(|row| row.vote == FeedbackVote::Stale)
        .count() as u32;
}

fn half_life(amount: f32, half: f32) -> f32 {
    0.5_f32.powf(amount / half)
}

fn elapsed_days(stamp: &str, now_secs: u64) -> f32 {
    let then = stamp.parse::<u64>().unwrap_or(now_secs);
    now_secs.saturating_sub(then) as f32 / DAY_SECONDS
}

impl crate::legacy::store::MemoryStore {
    pub fn feedback_for_account(
        &mut self,
        token: &str,
        memory_id: &str,
        vote: &str,
    ) -> Result<Memory, StoreError> {
        let vote = FeedbackVote::parse(vote)?;
        let account = self.authenticate_account(token)?;
        if !self
            .memories_for_account(token)?
            .iter()
            .any(|memory| memory.memory_id == memory_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown memory: {memory_id}"
            )));
        }
        let Some(memory) = self
            .state
            .memories
            .iter_mut()
            .find(|memory| memory.memory_id == memory_id)
        else {
            return Err(StoreError::Validation(format!(
                "unknown memory: {memory_id}"
            )));
        };
        apply_feedback(memory, &account.person_id, vote);
        let saved = memory.clone();
        self.persist()?;
        Ok(saved)
    }

    pub(crate) fn record_recall(&mut self, results: &[crate::legacy::RecallResult]) {
        if results.is_empty() {
            return;
        }
        let ids = results
            .iter()
            .map(|result| result.memory_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let now = now_seconds().to_string();
        for memory in &mut self.state.memories {
            if ids.contains(memory.memory_id.as_str()) {
                memory.access_count = memory.access_count.saturating_add(1);
                memory.last_accessed_at = Some(now.clone());
            }
        }
        // A full disk must not hide hits that were already ranked. persist restores
        // the last durable state, so the increment is simply not kept.
        let _ = self.persist();
    }
}

#[cfg(test)]
mod tests;
