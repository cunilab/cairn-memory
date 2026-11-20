use crate::legacy::jobs::{JobEnd, JobKind};
use crate::legacy::lifecycle::{MemoryKind, MemoryScope};
use crate::legacy::memories::Memory;
use crate::legacy::store::MemoryStore;
use crate::legacy::types::{StoreError, now_seconds};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

pub const CONTRADICTION_FLOOR: f32 = 0.25;
pub const SIMILARITY_BAND_MIN: f32 = 0.40;
pub const SIMILARITY_NEAR_DUPLICATE: f32 = 0.75;
pub const SIMILARITY_CANDIDATES: usize = 60;
pub const SIMILARITY_REVIEW_CAP: usize = 25;
pub const SIMILARITY_INTERVAL_SECONDS: u64 = 3600;
pub const SIMILARITY_JOB_KEY: &str = "similarity-scan";
const COSINE_SLOP: f32 = 1e-5;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SimilarityBand {
    PossibleConflict,
    NearDuplicate,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Open,
    Dismissed,
    Contradicted,
    Superseded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SimilarityReview {
    pub review_id: String,
    pub left_id: String,
    pub right_id: String,
    pub scope: MemoryScope,
    pub project_id: Option<String>,
    pub cosine: f32,
    pub band: SimilarityBand,
    pub status: ReviewStatus,
    pub created_at: u64,
    pub resolved_at: Option<u64>,
    pub resolved_by: Option<String>,
}

/// `1` with no live declared contradictions. Otherwise `max(0.25, 1/(1+n))`.
pub fn contradiction_factor(live: u32) -> f32 {
    if live == 0 {
        1.0
    } else {
        (1.0 / (1.0 + live as f32)).max(CONTRADICTION_FLOOR)
    }
}

/// Counts contradicting memories in `visible` that name `target` and share its scope and project.
pub fn contradiction_counts(visible: &[Memory]) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for other in visible {
        let mut seen = HashSet::new();
        for target_id in &other.contradicts {
            if !seen.insert(target_id.as_str()) {
                continue;
            }
            let Some(target) = visible.iter().find(|memory| memory.memory_id == *target_id) else {
                continue;
            };
            if other.memory_id == target.memory_id
                || other.scope != target.scope
                || other.project_id != target.project_id
            {
                continue;
            }
            *counts.entry(target_id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

pub fn similarity_band(cosine: f32) -> Option<SimilarityBand> {
    if !cosine.is_finite() || !(SIMILARITY_BAND_MIN..=1.0).contains(&cosine) {
        return None;
    }
    Some(if cosine < SIMILARITY_NEAR_DUPLICATE {
        SimilarityBand::PossibleConflict
    } else {
        SimilarityBand::NearDuplicate
    })
}

pub fn cosine(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (a, b) in left.iter().zip(right) {
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    let denom = left_norm.sqrt() * right_norm.sqrt();
    if denom <= 0.0 {
        return None;
    }
    let score = dot / denom;
    if !score.is_finite() {
        return None;
    }
    // A normalized vector dotted with itself can land a few ulps past 1.
    if (1.0..=1.0 + COSINE_SLOP).contains(&score) {
        return Some(1.0);
    }
    if (-1.0 - COSINE_SLOP..=-1.0).contains(&score) {
        return Some(-1.0);
    }
    Some(score)
}

pub fn ordered_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_owned(), right.to_owned())
    } else {
        (right.to_owned(), left.to_owned())
    }
}

fn review_id(left: &str, right: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((left.len() as u64).to_le_bytes());
    hasher.update(left.as_bytes());
    hasher.update((right.len() as u64).to_le_bytes());
    hasher.update(right.as_bytes());
    format!("review-{:x}", hasher.finalize())
        .chars()
        .take(31)
        .collect()
}

/// Pairs among the 60 coldest eligible memories. Captures, linked pairs, and stored pairs are skipped.
pub fn similarity_candidates(
    memories: &[Memory],
    vectors: &HashMap<String, Vec<f32>>,
    existing: &HashSet<(String, String)>,
    now: u64,
) -> Vec<SimilarityReview> {
    let mut eligible = memories
        .iter()
        .filter(|memory| memory.kind != MemoryKind::Capture)
        .filter(|memory| vectors.contains_key(&memory.memory_id))
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        left.access_count
            .cmp(&right.access_count)
            .then(left.created_at.cmp(&right.created_at))
            .then(left.memory_id.cmp(&right.memory_id))
    });
    eligible.truncate(SIMILARITY_CANDIDATES);
    let mut found = Vec::new();
    for (index, left) in eligible.iter().enumerate() {
        for right in eligible.iter().skip(index + 1) {
            if left.scope != right.scope || left.project_id != right.project_id {
                continue;
            }
            if left.supersedes.as_deref() == Some(right.memory_id.as_str())
                || right.supersedes.as_deref() == Some(left.memory_id.as_str())
                || left.contradicts.iter().any(|id| id == &right.memory_id)
                || right.contradicts.iter().any(|id| id == &left.memory_id)
            {
                continue;
            }
            let (pair_left, pair_right) = ordered_pair(&left.memory_id, &right.memory_id);
            if existing.contains(&(pair_left.clone(), pair_right.clone())) {
                continue;
            }
            let Some(value) = cosine(&vectors[&left.memory_id], &vectors[&right.memory_id]) else {
                continue;
            };
            let Some(band) = similarity_band(value) else {
                continue;
            };
            // Cosine alone is not evidence of a conflict. Only a near-duplicate
            // suggestion is published; the caller still decides what it means.
            if band != SimilarityBand::NearDuplicate {
                continue;
            }
            found.push(SimilarityReview {
                review_id: review_id(&pair_left, &pair_right),
                left_id: pair_left,
                right_id: pair_right,
                scope: left.scope.clone(),
                project_id: left.project_id.clone(),
                cosine: value,
                band,
                status: ReviewStatus::Open,
                created_at: now,
                resolved_at: None,
                resolved_by: None,
            });
        }
    }
    found.sort_by(|left, right| {
        right
            .cosine
            .partial_cmp(&left.cosine)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.left_id.cmp(&right.left_id))
            .then(left.right_id.cmp(&right.right_id))
    });
    found.truncate(SIMILARITY_REVIEW_CAP);
    found
}

pub fn same_scope(left: &Memory, right: &Memory) -> Result<(), StoreError> {
    if left.scope == right.scope && left.project_id == right.project_id {
        Ok(())
    } else {
        Err(StoreError::Validation(
            "contradiction must stay in the same scope and project".to_owned(),
        ))
    }
}

pub fn supersede_allowed(successor: &Memory, target: &Memory) -> Result<(), StoreError> {
    if successor.memory_id == target.memory_id {
        return Err(StoreError::Validation(
            "a memory cannot supersede itself".to_owned(),
        ));
    }
    if successor.scope != target.scope || successor.project_id != target.project_id {
        return Err(StoreError::Validation(
            "supersedes must stay in the same scope and project".to_owned(),
        ));
    }
    if let Some(existing) = successor.supersedes.as_deref()
        && existing != target.memory_id
    {
        return Err(StoreError::Validation(
            "successor already supersedes a different memory".to_owned(),
        ));
    }
    if target.supersedes.as_deref() == Some(successor.memory_id.as_str()) {
        return Err(StoreError::Validation("supersede would cycle".to_owned()));
    }
    Ok(())
}

impl MemoryStore {
    fn visible_memory(&self, token: &str, memory_id: &str) -> Result<Memory, StoreError> {
        self.memories_for_account(token)?
            .into_iter()
            .find(|memory| memory.memory_id == memory_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))
    }

    fn may_edit_both(&self, token: &str, left_id: &str, right_id: &str) -> Result<(), StoreError> {
        let account = self.authenticate_account(token)?;
        self.reject_member_mutation(&account, left_id)?;
        self.reject_member_mutation(&account, right_id)?;
        Ok(())
    }

    fn close_review(&mut self, left_id: &str, right_id: &str, status: ReviewStatus, person: &str) {
        let (left_id, right_id) = ordered_pair(left_id, right_id);
        let now = now_seconds();
        for review in &mut self.state.similarity_reviews {
            if review.status == ReviewStatus::Open
                && review.left_id == left_id
                && review.right_id == right_id
            {
                review.status = status;
                review.resolved_at = Some(now);
                review.resolved_by = Some(person.to_owned());
            }
        }
    }

    pub fn declare_contradiction(
        &mut self,
        token: &str,
        contradicting_id: &str,
        contradicted_id: &str,
    ) -> Result<Memory, StoreError> {
        if contradicting_id == contradicted_id {
            return Err(StoreError::Validation(
                "a memory cannot contradict itself".to_owned(),
            ));
        }
        let contradicting = self.visible_memory(token, contradicting_id)?;
        let contradicted = self.visible_memory(token, contradicted_id)?;
        same_scope(&contradicting, &contradicted)?;
        let account = self.authenticate_account(token)?;
        self.reject_member_mutation(&account, contradicted_id)?;
        let memory = self
            .state
            .memories
            .iter_mut()
            .find(|memory| memory.memory_id == contradicting_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {contradicting_id}")))?;
        if !memory.contradicts.iter().any(|id| id == contradicted_id) {
            memory.contradicts.push(contradicted_id.to_owned());
            memory.changed_by = Some(account.person_id.clone());
            memory.changed_at = Some(now_seconds().to_string());
        }
        let stored = memory.clone();
        self.close_review(
            contradicting_id,
            contradicted_id,
            ReviewStatus::Contradicted,
            &account.person_id,
        );
        self.persist()?;
        Ok(stored)
    }

    pub fn supersede_memory(
        &mut self,
        token: &str,
        successor_id: &str,
        target_id: &str,
    ) -> Result<Memory, StoreError> {
        let successor = self.visible_memory(token, successor_id)?;
        let target = self.visible_memory(token, target_id)?;
        supersede_allowed(&successor, &target)?;
        let account = self.authenticate_account(token)?;
        self.reject_member_mutation(&account, successor_id)?;
        self.reject_member_mutation(&account, target_id)?;
        let memory = self
            .state
            .memories
            .iter_mut()
            .find(|memory| memory.memory_id == successor_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {successor_id}")))?;
        memory.supersedes = Some(target_id.to_owned());
        memory.changed_by = Some(account.person_id.clone());
        memory.changed_at = Some(now_seconds().to_string());
        let stored = memory.clone();
        self.close_review(
            successor_id,
            target_id,
            ReviewStatus::Superseded,
            &account.person_id,
        );
        self.persist()?;
        Ok(stored)
    }

    pub fn similarity_reviews_for_account(
        &self,
        token: &str,
    ) -> Result<Vec<SimilarityReview>, StoreError> {
        let visible = self.memories_for_account(token)?;
        let ids = visible
            .iter()
            .map(|memory| memory.memory_id.as_str())
            .collect::<HashSet<_>>();
        let mut reviews = self
            .state
            .similarity_reviews
            .iter()
            .filter(|review| {
                review.status == ReviewStatus::Open
                    && ids.contains(review.left_id.as_str())
                    && ids.contains(review.right_id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        reviews.sort_by(|left, right| left.review_id.cmp(&right.review_id));
        Ok(reviews)
    }

    fn open_review(&self, token: &str, review_id: &str) -> Result<SimilarityReview, StoreError> {
        let review = self
            .similarity_reviews_for_account(token)?
            .into_iter()
            .find(|review| review.review_id == review_id)
            .ok_or_else(|| StoreError::Validation("unknown review".to_owned()))?;
        Ok(review)
    }

    pub fn dismiss_review(&mut self, token: &str, review_id: &str) -> Result<(), StoreError> {
        let review = self.open_review(token, review_id)?;
        self.may_edit_both(token, &review.left_id, &review.right_id)?;
        let account = self.authenticate_account(token)?;
        let now = now_seconds();
        let row = self
            .state
            .similarity_reviews
            .iter_mut()
            .find(|row| row.review_id == review_id)
            .ok_or_else(|| StoreError::Validation("unknown review".to_owned()))?;
        row.status = ReviewStatus::Dismissed;
        row.resolved_at = Some(now);
        row.resolved_by = Some(account.person_id);
        self.persist()?;
        Ok(())
    }

    pub fn contradict_review(
        &mut self,
        token: &str,
        review_id: &str,
        contradicted_id: &str,
    ) -> Result<Memory, StoreError> {
        let review = self.open_review(token, review_id)?;
        let contradicting_id = if review.left_id == contradicted_id {
            review.right_id
        } else if review.right_id == contradicted_id {
            review.left_id
        } else {
            return Err(StoreError::Validation(
                "contradicted memory is not in this review".to_owned(),
            ));
        };
        self.declare_contradiction(token, &contradicting_id, contradicted_id)
    }

    pub fn supersede_review(
        &mut self,
        token: &str,
        review_id: &str,
        successor_id: &str,
    ) -> Result<Memory, StoreError> {
        let review = self.open_review(token, review_id)?;
        let target_id = if review.left_id == successor_id {
            review.right_id
        } else if review.right_id == successor_id {
            review.left_id
        } else {
            return Err(StoreError::Validation(
                "successor memory is not in this review".to_owned(),
            ));
        };
        self.supersede_memory(token, successor_id, &target_id)
    }

    pub fn ensure_similarity_job(&mut self, now: u64) -> Result<(), StoreError> {
        let inserted = self
            .with_jobs(|book| book.enqueue(JobKind::Similarity, SIMILARITY_JOB_KEY, now, false));
        if inserted {
            self.persist()?;
        }
        Ok(())
    }

    fn stage_similarity_reviews(&mut self, now: u64) -> usize {
        let found = {
            let hidden = self.live_superseded_targets();
            let embeddings = self
                .state
                .embeddings
                .iter()
                .map(|embedding| (embedding.memory_id.as_str(), embedding))
                .collect::<HashMap<_, _>>();
            let mut coldest = BinaryHeap::new();
            for memory in &self.state.memories {
                if memory.kind == MemoryKind::Capture || !self.memory_is_recalled(memory, &hidden) {
                    continue;
                }
                let Some(embedding) = embeddings.get(memory.memory_id.as_str()) else {
                    continue;
                };
                if !embedding.matches(memory) {
                    continue;
                }
                coldest.push(ColdMemory(memory));
                if coldest.len() > SIMILARITY_CANDIDATES {
                    coldest.pop();
                }
            }
            let mut ranked = coldest
                .into_iter()
                .map(|memory| memory.0)
                .collect::<Vec<_>>();
            ranked.sort_by(|left, right| cold_order(left, right));
            let ids = ranked
                .iter()
                .map(|memory| memory.memory_id.as_str())
                .collect::<HashSet<_>>();
            let memories = ranked
                .iter()
                .map(|memory| (*memory).clone())
                .collect::<Vec<_>>();
            let vectors = ranked
                .iter()
                .filter_map(|memory| {
                    embeddings
                        .get(memory.memory_id.as_str())
                        .map(|embedding| (memory.memory_id.clone(), embedding.vector.clone()))
                })
                .collect::<HashMap<_, _>>();
            let existing = self
                .state
                .similarity_reviews
                .iter()
                .filter(|review| {
                    ids.contains(review.left_id.as_str()) && ids.contains(review.right_id.as_str())
                })
                .map(|review| (review.left_id.clone(), review.right_id.clone()))
                .collect::<HashSet<_>>();
            similarity_candidates(&memories, &vectors, &existing, now)
        };
        let added = found.len();
        self.state.similarity_reviews.extend(found);
        added
    }

    pub fn finish_similarity_scan(
        &mut self,
        claim: &crate::legacy::BackgroundJob,
        now: u64,
    ) -> Result<usize, StoreError> {
        let claim_id = claim.claim_id.clone().unwrap_or_default();
        let added = self.stage_similarity_reviews(now);
        let settled =
            self.with_jobs(|book| book.settle(&claim.job_id, &claim_id, now, JobEnd::Succeeded));
        if !settled {
            self.state = self.persisted.clone();
            return Ok(0);
        }
        self.with_jobs(|book| {
            book.enqueue(
                JobKind::Similarity,
                SIMILARITY_JOB_KEY,
                now.saturating_add(SIMILARITY_INTERVAL_SECONDS),
                false,
            );
        });
        self.persist()?;
        Ok(added)
    }
}

/// Warmer compares greater, so a bounded heap drops the warmest memory past the cap.
struct ColdMemory<'a>(&'a Memory);

fn cold_order(left: &Memory, right: &Memory) -> Ordering {
    left.access_count
        .cmp(&right.access_count)
        .then(left.created_at.cmp(&right.created_at))
        .then(left.memory_id.cmp(&right.memory_id))
}

impl Eq for ColdMemory<'_> {}

impl PartialEq for ColdMemory<'_> {
    fn eq(&self, other: &Self) -> bool {
        cold_order(self.0, other.0) == Ordering::Equal
    }
}

impl PartialOrd for ColdMemory<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ColdMemory<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        cold_order(self.0, other.0)
    }
}

#[cfg(test)]
mod tests;
