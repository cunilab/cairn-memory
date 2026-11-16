use crate::lifecycle::{Lifecycle, MemoryKind, MemoryScope, is_expired};
use crate::memories::Memory;
use crate::store::MemoryStore;
use crate::types::{StoreError, WorkContext};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_RELATIONSHIPS: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecallResult {
    #[serde(flatten)]
    pub fields: crate::MemoryFields,
    pub memory_id: String,
    pub content: String,
    pub source_event_ids: Vec<String>,
    pub context: WorkContext,
    pub lifecycle: Lifecycle,
    pub scope: MemoryScope,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub unresolved_work: Vec<String>,
    #[serde(default)]
    pub decision_reason: Option<String>,
    #[serde(default)]
    pub relationships: Vec<CodeRelationship>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub stale_reason: Option<String>,
    #[serde(default)]
    pub contradictory: bool,
    #[serde(default)]
    pub contradiction_ids: Vec<String>,
    #[serde(default)]
    pub promoted_from: Option<String>,
    #[serde(default)]
    pub promotion_reason: Option<String>,
    #[serde(default)]
    pub verified_commit: Option<String>,
    pub source: String,
    pub score: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeRelationship {
    pub relation: String,
    pub from: String,
    pub to: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContextPack {
    pub query: String,
    pub results: Vec<RecallResult>,
    pub omitted_count: usize,
    #[serde(default)]
    pub omitted_memory_ids: Vec<String>,
    #[serde(default)]
    pub recovery_hint: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecallFilter {
    #[serde(rename = "sessionId", alias = "session_id", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<WorkContext>,
}

impl MemoryStore {
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<RecallResult>, StoreError> {
        self.search_with_filter(query, limit, &RecallFilter::default())
    }

    pub fn search_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        let mut results = self.ranked_matches(query, filter)?;
        prefer_newer_contradiction(&mut results, &self.state.memories);
        results.truncate(limit);
        Ok(results)
    }

    /// Score-ranked matches before the contradiction tiebreak and the limit, so an account
    /// read can drop foreign ids first and break ties only on what it may see.
    fn ranked_matches(
        &self,
        query: &str,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        self.ranked_matches_in(query, filter, &self.state.memories)
    }

    fn ranked_matches_in(
        &self,
        query: &str,
        filter: &RecallFilter,
        memories: &[Memory],
    ) -> Result<Vec<RecallResult>, StoreError> {
        if query.trim().is_empty() {
            return Err(StoreError::Validation("query must not be empty".to_owned()));
        }

        let terms = tokenize(query);
        let superseded = superseded_targets(
            memories,
            &self.state.forgotten_ids,
            &self.memory_owner_map(),
        );
        let mut results: Vec<RecallResult> = memories
            .iter()
            .filter(|memory| self.memory_is_recalled(memory, &superseded))
            .filter(|memory| memory_matches_filter(memory, filter))
            .filter_map(|memory| {
                let score = score_memory(memory, &terms);
                (score > 0.0).then(|| {
                    recall_result_with(
                        memory,
                        score,
                        memories,
                        &self.state.forgotten_ids,
                        filter,
                        &superseded,
                    )
                })
            })
            .collect();

        results.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        prefer_decision(&mut results, &self.state.memories);
        Ok(results)
    }

    pub fn context(&self, query: &str, limit: usize) -> Result<ContextPack, StoreError> {
        self.context_with_filter(query, limit, &RecallFilter::default())
    }

    pub fn context_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_with_filter(query, usize::MAX, filter)?;
        Ok(build_context_pack(query.to_owned(), &mut results, limit))
    }

    fn retain_account_recall(&self, person_id: &str, results: &mut Vec<RecallResult>) {
        // Keep this account's rows, then drop contradiction ids it does not own.
        // Callers truncate or pack after this, so a foreign id cannot survive the limit.
        results.retain(|result| self.memory_owned_by(&result.memory_id, person_id));
        for result in results {
            result
                .contradiction_ids
                .retain(|id| self.memory_owned_by(id, person_id));
            result.contradictory = !result.contradiction_ids.is_empty();
        }
    }

    pub fn memories_for_account(&self, token: &str) -> Result<Vec<Memory>, StoreError> {
        let account = self.authenticate_account(token)?;
        let mut memories = self
            .state
            .memories
            .iter()
            .filter(|m| match m.scope {
                MemoryScope::Global => true,
                MemoryScope::Project if m.project_id.is_some() => m
                    .project_id
                    .as_deref()
                    .is_some_and(|id| self.require_project_reader(&account, id).is_ok()),
                _ => self.memory_owned_by(&m.memory_id, &account.person_id),
            })
            .cloned()
            .collect::<Vec<_>>();
        let owners = self.memory_owner_map();
        let hidden = superseded_targets(&memories, &self.state.forgotten_ids, &owners);
        memories = memories
            .iter()
            .filter(|m| self.memory_is_recalled(m, &hidden))
            .cloned()
            .collect();
        memories.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then(a.memory_id.cmp(&b.memory_id))
        });
        Ok(memories)
    }

    pub fn search_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let mut eligible = self
            .state
            .memories
            .iter()
            .filter(|m| {
                if m.scope == MemoryScope::Global {
                    return true;
                }
                if let Some(project_id) = m
                    .project_id
                    .as_deref()
                    .filter(|_| m.scope == MemoryScope::Project)
                {
                    self.project_owner_id(project_id).is_some()
                        && (self.project_owner_id(project_id).as_deref()
                            == Some(account.person_id.as_str())
                            || self.state.project_members.iter().any(|r| {
                                r.project_id == project_id
                                    && r.member_person_id == account.person_id
                            }))
                } else {
                    self.memory_owned_by(&m.memory_id, &account.person_id)
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        for m in &mut eligible {
            if m.published_by.is_some() {
                m.promoted_from = None;
            }
        }
        let mut results = self.ranked_matches_in(query, filter, &eligible)?;
        prefer_newer_contradiction(&mut results, &eligible);
        results.truncate(limit);
        Ok(results)
    }

    pub fn context_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_for_account(account_token, query, usize::MAX, filter)?;
        Ok(build_context_pack(query.to_owned(), &mut results, limit))
    }

    pub fn resume_for_account(
        &self,
        account_token: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let mut results = self.resume_context(usize::MAX, filter)?;
        self.retain_account_recall(&account.person_id, &mut results.results);
        Ok(build_context_pack(
            "resume".to_owned(),
            &mut results.results,
            limit,
        ))
    }

    pub fn unresolved_for_account(
        &self,
        account_token: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let mut results = self.unresolved_context(usize::MAX, filter)?;
        self.retain_account_recall(&account.person_id, &mut results.results);
        Ok(build_context_pack(
            "unresolved".to_owned(),
            &mut results.results,
            limit,
        ))
    }

    pub fn why_changed_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_for_account(account_token, query, usize::MAX, filter)?;
        results.sort_by(|left, right| {
            right
                .decision_reason
                .is_some()
                .cmp(&left.decision_reason.is_some())
                .then_with(|| right.relationships.len().cmp(&left.relationships.len()))
                .then_with(|| {
                    right
                        .score
                        .partial_cmp(&left.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        Ok(build_context_pack(
            format!("why changed: {query}"),
            &mut results,
            limit,
        ))
    }

    pub fn resume_context(
        &self,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let superseded = self.live_superseded_targets();
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .rev()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| self.memory_is_recalled(memory, &superseded))
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| {
                recall_result_with(
                    memory,
                    1.0,
                    &self.state.memories,
                    &self.state.forgotten_ids,
                    filter,
                    &superseded,
                )
            })
            .collect();
        Ok(build_context_pack("resume".to_owned(), &mut results, limit))
    }

    pub fn unresolved_context(
        &self,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let superseded = self.live_superseded_targets();
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .rev()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| !memory.unresolved_work.is_empty())
            .filter(|memory| self.memory_is_recalled(memory, &superseded))
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| {
                recall_result_with(
                    memory,
                    1.0,
                    &self.state.memories,
                    &self.state.forgotten_ids,
                    filter,
                    &superseded,
                )
            })
            .collect();
        Ok(build_context_pack(
            "unresolved".to_owned(),
            &mut results,
            limit,
        ))
    }

    pub fn why_changed_context(
        &self,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_with_filter(query, usize::MAX, filter)?;
        results.sort_by(|left, right| {
            right
                .decision_reason
                .is_some()
                .cmp(&left.decision_reason.is_some())
                .then_with(|| right.relationships.len().cmp(&left.relationships.len()))
                .then_with(|| {
                    right
                        .score
                        .partial_cmp(&left.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        Ok(build_context_pack(
            format!("why changed: {query}"),
            &mut results,
            limit,
        ))
    }

    pub(crate) fn memory_is_forgotten(&self, memory: &Memory) -> bool {
        memory_forgotten(memory, &self.state.forgotten_ids)
    }

    /// Ids a live successor hides, built once per request. The owner map is the only
    /// place file ownership enters supersession; see `superseded_targets`.
    pub(crate) fn live_superseded_targets(&self) -> HashSet<&str> {
        superseded_targets(
            &self.state.memories,
            &self.state.forgotten_ids,
            &self.memory_owner_map(),
        )
    }

    pub(crate) fn memory_owner_map(&self) -> HashMap<&str, &str> {
        let mut owners = HashMap::with_capacity(self.state.memory_owners.len());
        for record in &self.state.memory_owners {
            // First record wins, like `item_owner`.
            owners
                .entry(record.memory_id.as_str())
                .or_insert(record.person_id.as_str());
        }
        owners
    }

    /// Explicit publications only. A private/imported project tag is never authority.
    pub(crate) fn project_rows(&self, project_id: &str) -> Vec<Memory> {
        self.state
            .memories
            .iter()
            .filter(|m| m.project_id.as_deref() == Some(project_id) && m.published_by.is_some())
            .cloned()
            .collect()
    }

    pub(crate) fn memory_is_recalled(&self, memory: &Memory, superseded: &HashSet<&str>) -> bool {
        memory.fields.status != crate::MemoryStatus::Superseded
            && !self.memory_is_forgotten(memory)
            && !is_expired(&memory.lifecycle)
            && !superseded.contains(memory.memory_id.as_str())
    }
}

pub fn build_context_pack(
    query: String,
    results: &mut Vec<RecallResult>,
    limit: usize,
) -> ContextPack {
    let omitted_count = results.len().saturating_sub(limit);
    let omitted_memory_ids = results
        .iter()
        .skip(limit)
        .map(|result| result.memory_id.clone())
        .collect::<Vec<_>>();
    results.truncate(limit);
    ContextPack {
        query,
        omitted_count,
        recovery_hint: (omitted_count > 0).then(|| {
            "Increase the limit or query one of the omitted memory IDs to recover additional context."
                .to_owned()
        }),
        omitted_memory_ids,
        results: std::mem::take(results),
    }
}

fn decision_ids(memories: &[Memory]) -> HashSet<&str> {
    memories
        .iter()
        .filter(|memory| memory.kind == MemoryKind::Decision)
        .map(|memory| memory.memory_id.as_str())
        .collect()
}

/// Among results with equal scores, a decision (project decision) ranks before any other
/// memory. Scores are unchanged and the stable sort keeps the earlier order otherwise.
pub fn prefer_decision(results: &mut [RecallResult], memories: &[Memory]) {
    let decisions = decision_ids(memories);
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                decisions
                    .contains(right.memory_id.as_str())
                    .cmp(&decisions.contains(left.memory_id.as_str()))
            })
    });
}

/// Among results with equal scores that list each other as contradictions, the newer
/// memory ranks first, unless that would put a non-decision before a decision. Nothing is
/// resolved or hidden. Adjacent swaps only, so unrelated order is unchanged and no
/// comparator has to be a total order.
pub fn prefer_newer_contradiction(results: &mut [RecallResult], memories: &[Memory]) {
    let created: HashMap<&str, &str> = memories
        .iter()
        .map(|memory| (memory.memory_id.as_str(), memory.created_at.as_str()))
        .collect();
    let decisions = decision_ids(memories);
    let older_first = |earlier: &RecallResult, later: &RecallResult| {
        earlier.score == later.score
            && decisions.contains(earlier.memory_id.as_str())
                == decisions.contains(later.memory_id.as_str())
            && (earlier.contradiction_ids.contains(&later.memory_id)
                || later.contradiction_ids.contains(&earlier.memory_id))
            && created.get(earlier.memory_id.as_str()) < created.get(later.memory_id.as_str())
    };
    for index in 1..results.len() {
        let mut at = index;
        while at > 0 && older_first(&results[at - 1], &results[at]) {
            results.swap(at - 1, at);
            at -= 1;
        }
    }
}

pub fn recall_result(
    memory: &Memory,
    score: f32,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    filter: &RecallFilter,
) -> RecallResult {
    // The slice has no ownership data, so every live successor counts. PostgreSQL passes
    // one person's rows, which is the same rule.
    let superseded = superseded_targets(all_memories, forgotten_ids, &HashMap::new());
    recall_result_with(
        memory,
        score,
        all_memories,
        forgotten_ids,
        filter,
        &superseded,
    )
}

fn recall_result_with(
    memory: &Memory,
    score: f32,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    filter: &RecallFilter,
    superseded_ids: &HashSet<&str>,
) -> RecallResult {
    let (stale, stale_reason) = stale_signal(memory, filter);
    let contradiction_ids = contradiction_ids(memory, all_memories, forgotten_ids, superseded_ids)
        .into_iter()
        .filter(|id| {
            all_memories
                .iter()
                .any(|m| m.memory_id == *id && memory_matches_filter(m, filter))
        })
        .collect::<Vec<_>>();
    RecallResult {
        fields: memory.fields.clone(),
        memory_id: memory.memory_id.clone(),
        content: memory.content.clone(),
        source_event_ids: memory.source_event_ids.clone(),
        context: memory.context.clone(),
        lifecycle: memory.lifecycle.clone(),
        scope: memory.scope.clone(),
        project_id: memory.project_id.clone(),
        next_steps: memory.next_steps.clone(),
        unresolved_work: memory.unresolved_work.clone(),
        decision_reason: (!memory.fields.reason.is_empty())
            .then(|| memory.fields.reason.clone())
            .or_else(|| extract_decision_reason(&memory.content)),
        relationships: code_relationships(memory),
        stale,
        stale_reason,
        contradictory: !contradiction_ids.is_empty(),
        contradiction_ids,
        promoted_from: live_origin(memory, all_memories, forgotten_ids, superseded_ids),
        promotion_reason: memory.promotion_reason.clone(),
        verified_commit: memory.verified_commit.clone(),
        source: match memory.scope {
            MemoryScope::Project => "project",
            MemoryScope::Global => "global",
            _ => "personal",
        }
        .to_owned(),
        score,
    }
}

/// The personal memory a promoted copy came from, only while that memory is still recalled.
/// Once it is forgotten, expired or superseded the link is inert: stored, never exposed.
pub(crate) fn live_origin(
    memory: &Memory,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    superseded_ids: &HashSet<&str>,
) -> Option<String> {
    let origin = memory.promoted_from.as_deref()?;
    all_memories
        .iter()
        .find(|candidate| {
            candidate.memory_id == origin
                && !memory_forgotten(candidate, forgotten_ids)
                && !is_expired(&candidate.lifecycle)
                && !superseded_ids.contains(origin)
        })
        .map(|candidate| candidate.memory_id.clone())
}

/// Why-changed priority of a memory: whether it carries a decision reason, then its
/// relationship count. These are the same helpers `recall_result` fills in, so a store can
/// rank before building results.
pub fn why_changed_priority(memory: &Memory) -> (bool, usize) {
    (
        !memory.fields.reason.is_empty() || extract_decision_reason(&memory.content).is_some(),
        code_relationships(memory).len(),
    )
}

fn extract_decision_reason(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let start = lower
        .find("because")
        .map(|index| index + "because".len())
        .or_else(|| lower.find("reason:").map(|index| index + "reason:".len()))?;
    let reason = content[start..]
        .split_once("[evidence:")
        .map(|(before, _)| before)
        .unwrap_or(&content[start..])
        .trim()
        .trim_start_matches([':', '-', ' '])
        .trim_end_matches(['.', '!', '?'])
        .trim();
    (!reason.is_empty()).then(|| reason.to_owned())
}

fn code_relationships(memory: &Memory) -> Vec<CodeRelationship> {
    let mut relationships = Vec::new();
    let memory_id = memory.memory_id.as_str();
    for file in &memory.context.files {
        push_relationship(
            &mut relationships,
            "memory_touches_file",
            memory_id,
            file,
            "captured work context",
        );
    }
    for symbol in &memory.context.symbols {
        push_relationship(
            &mut relationships,
            "memory_mentions_symbol",
            memory_id,
            symbol,
            "captured work context",
        );
        for file in &memory.context.files {
            push_relationship(
                &mut relationships,
                "file_contains_symbol",
                file,
                symbol,
                "same observation context",
            );
        }
        for test in &memory.context.tests {
            push_relationship(
                &mut relationships,
                "symbol_verified_by_test",
                symbol,
                test,
                "same observation context",
            );
        }
    }
    for command in &memory.context.commands {
        for test in &memory.context.tests {
            push_relationship(
                &mut relationships,
                "command_verifies_test",
                command,
                test,
                "same observation context",
            );
        }
    }
    for service in &memory.context.services {
        for file in &memory.context.files {
            push_relationship(
                &mut relationships,
                "service_owns_file",
                service,
                file,
                "same observation context",
            );
        }
    }
    relationships
}

fn push_relationship(
    relationships: &mut Vec<CodeRelationship>,
    relation: &str,
    from: &str,
    to: &str,
    evidence: &str,
) {
    if relationships.len() >= MAX_RELATIONSHIPS {
        return;
    }
    relationships.push(CodeRelationship {
        relation: relation.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
        evidence: evidence.to_owned(),
    });
}

fn stale_signal(memory: &Memory, filter: &RecallFilter) -> (bool, Option<String>) {
    if memory.fields.status == crate::MemoryStatus::Stale || memory.lifecycle.freshness < 50 {
        return (true, Some("memory freshness is below 50".to_owned()));
    }
    let Some(current) = filter.context.as_ref() else {
        return (false, None);
    };
    if memory.scope == MemoryScope::Personal
        && memory.context.repository == current.repository
        && memory.context.commit.is_some()
        && current.commit.is_some()
        && memory.context.commit != current.commit
    {
        return (
            true,
            Some("memory was captured at a different commit".to_owned()),
        );
    }
    if memory.scope == MemoryScope::Project
        && memory.context.repository == current.repository
        && memory.verified_commit.is_some()
        && current.commit.is_some()
        && memory.verified_commit != current.commit
    {
        return (
            true,
            Some("project memory was verified at a different commit".to_owned()),
        );
    }
    (false, None)
}

fn memory_forgotten(memory: &Memory, forgotten_ids: &HashSet<String>) -> bool {
    forgotten_ids.contains(&memory.memory_id)
        || memory
            .source_event_ids
            .iter()
            .any(|event_id| forgotten_ids.contains(event_id))
}

fn live_superseded_target<'a>(
    candidate: &'a Memory,
    forgotten_ids: &HashSet<String>,
) -> Option<&'a str> {
    let target = candidate.supersedes.as_deref()?;
    (!memory_forgotten(candidate, forgotten_ids)
        && !is_expired(&candidate.lifecycle)
        && candidate.fields.status != crate::MemoryStatus::Superseded)
        .then_some(target)
}

/// Ids hidden by a live successor in `memories`. A link whose successor has an owner that
/// differs from the target's owner (or the target has none) is stored but does not count:
/// an account cannot hide another account's memory. A successor with no owner is local
/// or pre-account data and counts. Pass an empty map when the slice has no ownership.
pub(crate) fn superseded_targets<'a>(
    memories: &'a [Memory],
    forgotten_ids: &HashSet<String>,
    owners: &HashMap<&str, &str>,
) -> HashSet<&'a str> {
    let publications = memories
        .iter()
        .filter(|m| m.published_by.is_some())
        .map(|m| (m.memory_id.as_str(), m.project_id.as_deref()))
        .collect::<HashMap<_, _>>();
    memories
        .iter()
        .filter_map(|candidate| {
            let target = live_superseded_target(candidate, forgotten_ids)?;
            if memories.iter().any(|m| {
                m.memory_id == target
                    && (m.scope != candidate.scope || m.project_id != candidate.project_id)
            }) {
                return None;
            }
            let project_correction = candidate.published_by.is_some()
                && publications
                    .get(target)
                    .is_some_and(|project| *project == candidate.project_id.as_deref());
            if publications.contains_key(target) && !project_correction {
                return None;
            }
            let foreign = owners
                .get(candidate.memory_id.as_str())
                .is_some_and(|successor| owners.get(target) != Some(successor));
            let global_correction = candidate.scope == MemoryScope::Global
                && memories
                    .iter()
                    .any(|m| m.memory_id == target && m.scope == MemoryScope::Global);
            (!foreign || project_correction || global_correction).then_some(target)
        })
        .collect()
}

pub(crate) fn contradiction_ids(
    memory: &Memory,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    superseded_ids: &HashSet<&str>,
) -> Vec<String> {
    let target_negative = contains_negation(&memory.content);
    let target_topics = topic_tokens(&memory.content);
    if target_topics.is_empty() {
        return Vec::new();
    }
    all_memories
        .iter()
        .filter(|other| other.memory_id != memory.memory_id)
        .filter(|other| {
            !memory_forgotten(other, forgotten_ids)
                && !is_expired(&other.lifecycle)
                && !superseded_ids.contains(other.memory_id.as_str())
        })
        .filter(|other| same_scope_context(memory, other))
        .filter(|other| contains_negation(&other.content) != target_negative)
        .filter(|other| {
            let other_topics = topic_tokens(&other.content);
            target_topics.intersection(&other_topics).count() >= 2
        })
        .map(|other| other.memory_id.clone())
        .take(8)
        .collect()
}

fn same_scope_context(left: &Memory, right: &Memory) -> bool {
    if left.published_by.is_some()
        && right.published_by.is_some()
        && left.project_id != right.project_id
    {
        return false;
    }
    if left.scope != right.scope {
        return false;
    }
    match left.scope {
        MemoryScope::Global => true,
        MemoryScope::Project => left.project_id == right.project_id,
        MemoryScope::Personal => {
            left.context.repository == right.context.repository
                && left.context.branch == right.context.branch
                && left.context.worktree == right.context.worktree
        }
    }
}

fn contains_negation(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "do not ",
        "does not ",
        "don't ",
        "never ",
        "avoid ",
        "not use ",
        "not the ",
        "without ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn topic_tokens(content: &str) -> HashSet<String> {
    let stop_words = [
        "the", "and", "for", "with", "use", "default", "should", "this", "that", "because", "not",
        "do", "does", "is", "are", "as", "to", "a", "an", "of", "in",
    ];
    tokenize(content)
        .into_iter()
        .filter(|term| !stop_words.contains(&term.as_str()))
        .collect()
}

pub fn memory_matches_filter(memory: &Memory, filter: &RecallFilter) -> bool {
    if filter.session_id.is_none() && filter.context.is_none() {
        return true;
    }
    match memory.scope {
        MemoryScope::Global => true,
        MemoryScope::Project => filter
            .context
            .as_ref()
            .and_then(|c| c.project_id.as_ref())
            .is_some_and(|id| memory.project_id.as_ref() == Some(id)),
        MemoryScope::Personal => {
            if !matches!(
                memory.kind,
                MemoryKind::SessionSummary | MemoryKind::Capture
            ) {
                return true;
            }
            if let Some(id) = &filter.session_id
                && memory.session_id != *id
            {
                return false;
            }
            filter.context.as_ref().is_none_or(|c| {
                c.repository
                    .as_ref()
                    .is_none_or(|r| memory.context.repository.as_ref() == Some(r))
                    && c.branch
                        .as_ref()
                        .is_none_or(|r| memory.context.branch.as_ref() == Some(r))
                    && c.worktree
                        .as_ref()
                        .is_none_or(|r| memory.context.worktree.as_ref() == Some(r))
            })
        }
    }
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| term.len() >= 2)
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

fn score_memory(memory: &Memory, terms: &[String]) -> f32 {
    let haystack = format!(
        "{} {} {} {} {} {} {} {} {}",
        memory.content,
        memory.context.repository.as_deref().unwrap_or_default(),
        memory.context.branch.as_deref().unwrap_or_default(),
        memory.context.worktree.as_deref().unwrap_or_default(),
        memory.context.files.join(" "),
        memory.context.symbols.join(" "),
        memory.context.commands.join(" "),
        memory.context.tests.join(" "),
        memory.context.services.join(" "),
    )
    .to_ascii_lowercase();

    let title = memory.fields.title.to_ascii_lowercase();
    terms
        .iter()
        .map(|term| {
            let count = haystack.matches(term).count() + 3 * title.matches(term).count();
            count as f32
        })
        .sum()
}

#[cfg(test)]
mod tests;
