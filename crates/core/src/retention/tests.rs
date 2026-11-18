use super::*;
use crate::lifecycle::{MemoryKind, MemoryScope};
use crate::memories::{Memory, SaveMemoryInput, build_saved_memory};
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{join_project, save_project_memory, temp_store_path};
use crate::types::{WorkContext, now_seconds};
use serde_json::json;
use std::fs;

fn memory(kind: MemoryKind, age_days: u64, access: u64, importance: u8) -> Memory {
    let now = 1_700_000_000_u64;
    let mut memory = build_saved_memory(
        serde_json::from_value::<SaveMemoryInput>(json!({
            "scope": "personal",
            "kind": kind,
            "title": "Title",
            "summary": "Summary",
            "reason": "Reason",
            "importance": importance
        }))
        .unwrap(),
        "alice",
    )
    .unwrap();
    memory.created_at = now.saturating_sub(age_days * 86_400).to_string();
    memory.access_count = access;
    memory.last_accessed_at = (access > 0).then(|| memory.created_at.clone());
    if access == 0 {
        memory.last_accessed_at = None;
    }
    memory
}

fn at(_memory: &Memory) -> u64 {
    1_700_000_000
}

#[test]
fn constants_match_the_worked_examples() {
    let fresh = memory(MemoryKind::Capture, 0, 0, 50);
    assert!((retention_score(&fresh, at(&fresh)) - 0.625).abs() < 1e-4);
    assert_eq!(retention_factor(&fresh, at(&fresh)), 1.0);

    let unused = memory(MemoryKind::Capture, 60, 0, 50);
    let unused_score = retention_score(&unused, at(&unused));
    assert!((unused_score - 0.2477).abs() < 1e-3, "{unused_score}");
    let factor = retention_factor(&unused, at(&unused));
    assert!((factor - 0.663).abs() < 1e-2, "{factor}");
    assert!((BELOW_THRESHOLD_FLOOR..1.0).contains(&factor));

    let mut reinforced = memory(MemoryKind::Capture, 60, 8, 50);
    reinforced.last_accessed_at = Some(at(&reinforced).to_string());
    let reinforced_score = retention_score(&reinforced, at(&reinforced));
    assert!(
        (reinforced_score - 0.5715).abs() < 1e-3,
        "{reinforced_score}"
    );
    assert_eq!(retention_factor(&reinforced, at(&reinforced)), 1.0);
}

#[test]
fn purposeful_saves_do_not_decay_and_feedback_is_bounded() {
    for kind in [
        MemoryKind::Rule,
        MemoryKind::Decision,
        MemoryKind::Gotcha,
        MemoryKind::Procedure,
        MemoryKind::Fact,
        MemoryKind::Preference,
        MemoryKind::Explicit,
        MemoryKind::Lesson,
        MemoryKind::Correction,
    ] {
        let aged = memory(kind, 400, 0, 1);
        assert!(!decays(kind), "{kind:?}");
        assert_eq!(retention_factor(&aged, at(&aged)), 1.0);
    }
    for kind in [
        MemoryKind::Capture,
        MemoryKind::SessionSummary,
        MemoryKind::Handoff,
    ] {
        assert!(decays(kind), "{kind:?}");
    }
    assert_eq!(feedback_factor(0, 0), 1.0);
    assert!((feedback_factor(1, 0) - 1.15).abs() < 1e-6);
    assert_eq!(feedback_factor(4, 0), 1.0 + FEEDBACK_CAP);
    assert_eq!(feedback_factor(0, 4), 1.0 - FEEDBACK_CAP);
    assert_eq!(feedback_factor(2, 2), 1.0);
}

#[test]
fn recall_reinforces_only_returned_hits_and_never_deletes() -> Result<(), StoreError> {
    let path = temp_store_path("retention-reinforce");
    let mut store = MemoryStore::open(&path)?;
    let account = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let now = now_seconds();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let saved = store.save_for_account(
            &account.token,
            serde_json::from_value(json!({
                "scope": "personal",
                "kind": "capture",
                "title": "decaytoken shared",
                "summary": "decaytoken shared",
                "reason": "verified"
            }))?,
        )?;
        ids.push(saved.memory_id);
    }
    let old = &ids[0];
    {
        let row = store
            .state
            .memories
            .iter_mut()
            .find(|memory| &memory.memory_id == old)
            .unwrap();
        row.created_at = now.saturating_sub(120 * 86_400).to_string();
        row.lifecycle.importance = 50;
    }
    let before = store.state.memories.len();
    let hits =
        store.search_for_account(&account.token, "decaytoken", 10, &RecallFilter::default())?;
    assert_eq!(store.state.memories.len(), before);
    assert_eq!(hits.len(), 2);
    let fresh = hits.iter().find(|hit| hit.memory_id != *old).unwrap();
    let aged = hits.iter().find(|hit| hit.memory_id == *old).unwrap();
    assert!(fresh.score > aged.score);
    assert!(
        store
            .state
            .memories
            .iter()
            .any(|memory| memory.memory_id == *old)
    );

    let top =
        store.search_for_account(&account.token, "decaytoken", 1, &RecallFilter::default())?;
    assert_eq!(top.len(), 1);
    let top_count = store
        .state
        .memories
        .iter()
        .find(|memory| memory.memory_id == top[0].memory_id)
        .unwrap()
        .access_count;
    let other_count = store
        .state
        .memories
        .iter()
        .find(|memory| memory.memory_id != top[0].memory_id && ids.contains(&memory.memory_id))
        .unwrap()
        .access_count;
    assert_eq!(top_count, 2);
    assert_eq!(other_count, 1);

    let pack =
        store.context_for_account(&account.token, "decaytoken", 1, &RecallFilter::default())?;
    assert_eq!(pack.omitted_count, 1);
    assert_eq!(pack.results.len(), 1);
    let omitted = &pack.omitted_memory_ids[0];
    let omitted_count = store
        .state
        .memories
        .iter()
        .find(|memory| &memory.memory_id == omitted)
        .unwrap()
        .access_count;
    assert_eq!(
        omitted_count,
        if omitted == &top[0].memory_id { 2 } else { 1 }
    );

    let summary = store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.memory_id == *old)
        .unwrap();
    summary.kind = MemoryKind::SessionSummary;
    let summary_before = summary.access_count;
    let resumed = store.resume_context(5, &RecallFilter::default())?;
    assert!(resumed.results.iter().any(|hit| hit.memory_id == *old));
    let summary_after = store
        .state
        .memories
        .iter()
        .find(|memory| memory.memory_id == *old)
        .unwrap()
        .access_count;
    assert_eq!(summary_after, summary_before);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn aged_rules_keep_stage3_weight_and_feedback_reorders_without_hiding() -> Result<(), StoreError> {
    let path = temp_store_path("retention-feedback");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let now = now_seconds();
    let mut rule_ids = Vec::new();
    for _ in 0..2 {
        let saved = store.save_for_account(
            &owner.token,
            serde_json::from_value(json!({
                "scope": "personal",
                "kind": "rule",
                "title": "ruletoken shared",
                "summary": "ruletoken shared",
                "reason": "verified",
                "rule": "keep"
            }))?,
        )?;
        rule_ids.push(saved.memory_id);
    }
    {
        let row = store
            .state
            .memories
            .iter_mut()
            .find(|memory| memory.memory_id == rule_ids[0])
            .unwrap();
        row.created_at = now.saturating_sub(400 * 86_400).to_string();
    }
    let rules =
        store.search_for_account(&owner.token, "ruletoken", 10, &RecallFilter::default())?;
    let high = rules.iter().map(|hit| hit.score).fold(0.0_f32, f32::max);
    let low = rules.iter().map(|hit| hit.score).fold(f32::MAX, f32::min);
    assert!(high / low < 1.2, "aged rule decayed: {high} / {low}");

    let mut twins = Vec::new();
    for _ in 0..2 {
        let saved = store.save_for_account(
            &owner.token,
            serde_json::from_value(json!({
                "scope": "personal",
                "kind": "capture",
                "title": "feedtoken shared",
                "summary": "feedtoken shared",
                "reason": "verified"
            }))?,
        )?;
        twins.push(saved.memory_id);
    }
    store.feedback_for_account(&owner.token, &twins[0], "helpful")?;
    store.feedback_for_account(&owner.token, &twins[0], "helpful")?;
    let voted = store
        .state
        .memories
        .iter()
        .find(|memory| memory.memory_id == twins[0])
        .unwrap();
    assert_eq!(voted.helpful_count, 1);
    assert_eq!(voted.feedback.len(), 1);
    let stale = store.feedback_for_account(&owner.token, &twins[0], "stale")?;
    assert_eq!((stale.helpful_count, stale.stale_count), (0, 1));
    assert_eq!(stale.feedback.len(), 1);
    assert_eq!(stale.feedback[0].vote, FeedbackVote::Stale);
    store.feedback_for_account(&owner.token, &twins[0], "helpful")?;
    let hits = store.search_for_account(&owner.token, "feedtoken", 10, &RecallFilter::default())?;
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].memory_id, twins[0]);
    assert!(hits[0].score > hits[1].score);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn feedback_follows_visibility_and_one_vote_per_person() -> Result<(), StoreError> {
    let path = temp_store_path("retention-visibility");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let personal = store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "personal",
            "kind": "preference",
            "title": "privatepref",
            "summary": "privatepref",
            "reason": "verified",
            "preference": "short"
        }))?,
    )?;
    let refused = store
        .feedback_for_account(&other.token, &personal.memory_id, "stale")
        .expect_err("personal feedback is owner-only");
    assert!(refused.to_string().contains("unknown memory"));
    assert!(
        store
            .state
            .memories
            .iter()
            .any(|memory| memory.memory_id == personal.memory_id)
    );

    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    let shared = save_project_memory(
        &mut store,
        &owner.token,
        &project.project_id,
        "sharedtoken",
        WorkContext {
            project_id: Some(project.project_id.clone()),
            ..WorkContext::default()
        },
    )?;
    let outsider = store
        .feedback_for_account(&other.token, &shared.memory_id, "helpful")
        .expect_err("non-member");
    assert!(outsider.to_string().contains("unknown memory"));
    join_project(&mut store, &other.token, &project.project_id)?;
    let before =
        store.search_for_account(&owner.token, "sharedtoken", 5, &RecallFilter::default())?;
    let voted = store.feedback_for_account(&other.token, &shared.memory_id, "helpful")?;
    assert_eq!(voted.helpful_count, 1);
    let seen =
        store.search_for_account(&owner.token, "sharedtoken", 5, &RecallFilter::default())?;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].score > before[0].score);

    let global = store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "global",
            "kind": "fact",
            "title": "globaltoken",
            "summary": "globaltoken",
            "reason": "verified",
            "statement": "holds"
        }))?,
    )?;
    let global_vote = store.feedback_for_account(&other.token, &global.memory_id, "stale")?;
    assert_eq!(global_vote.stale_count, 1);
    assert_eq!(global_vote.scope, MemoryScope::Global);
    let bad = store
        .feedback_for_account(&owner.token, &global.memory_id, "maybe")
        .expect_err("vote");
    assert!(bad.to_string().contains("helpful or stale"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn feedback_rejects_every_memory_hidden_from_reads() -> Result<(), StoreError> {
    let path = temp_store_path("feedback-hidden");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    for hidden in [
        "forgotten",
        "source-forgotten",
        "superseded",
        "expired",
        "successor",
    ] {
        let saved = store.save_for_account(
            &owner.token,
            serde_json::from_value(json!({
                "scope": "global", "kind": "fact", "title": hidden,
                "summary": hidden, "reason": "verified", "statement": "holds"
            }))?,
        )?;
        match hidden {
            "forgotten" => store.forget_for_account(&owner.token, &saved.memory_id)?,
            "source-forgotten" => {
                store
                    .state
                    .forgotten_ids
                    .insert(saved.source_event_ids[0].clone());
            }
            "superseded" => {
                store
                    .state
                    .memories
                    .iter_mut()
                    .find(|m| m.memory_id == saved.memory_id)
                    .unwrap()
                    .fields
                    .status = crate::MemoryStatus::Superseded
            }
            "expired" => {
                store
                    .state
                    .memories
                    .iter_mut()
                    .find(|m| m.memory_id == saved.memory_id)
                    .unwrap()
                    .lifecycle
                    .expires_at = Some("1".to_owned())
            }
            "successor" => {
                let mut successor = saved.clone();
                successor.memory_id.push_str("-successor");
                successor.supersedes = Some(saved.memory_id.clone());
                store.record_memory_owner(&owner.account.person_id, &successor.memory_id);
                store.state.memories.push(successor);
            }
            _ => unreachable!(),
        }
        assert!(
            !store
                .memories_for_account(&other.token)?
                .iter()
                .any(|m| m.memory_id == saved.memory_id),
            "{hidden}"
        );
        for token in [&owner.token, &other.token] {
            let error = store
                .feedback_for_account(token, &saved.memory_id, "helpful")
                .expect_err(hidden);
            assert_eq!(
                error.to_string(),
                format!("validation error: unknown memory: {}", saved.memory_id)
            );
        }
        assert!(
            store
                .state
                .memories
                .iter()
                .find(|m| m.memory_id == saved.memory_id)
                .unwrap()
                .feedback
                .is_empty()
        );
    }
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn summary_regeneration_preserves_reinforcement_and_feedback() -> Result<(), StoreError> {
    let path = temp_store_path("summary-feedback");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    store.start_session_for_account(&owner.token, "summary-feedback", WorkContext::default())?;
    store.capture_for_account(
        &owner.token,
        crate::test_support::observation_for(
            "summary-feedback",
            "first",
            "summaryneedle initial capture",
        ),
    )?;
    let summary = store.end_session_for_account(&owner.token, "summary-feedback")?;
    for (event, vote) in [("second", "stale"), ("third", "helpful")] {
        store.feedback_for_account(&owner.token, &summary.memory_id, vote)?;
        store.search_for_account(&owner.token, "summaryneedle", 20, &RecallFilter::default())?;
        let before = store
            .memories_for_account(&owner.token)?
            .into_iter()
            .find(|m| m.memory_id == summary.memory_id)
            .unwrap();
        assert!(before.access_count > 0);
        assert!(before.last_accessed_at.is_some());
        assert_eq!(before.feedback.len(), 1);
        store.capture_for_account(
            &owner.token,
            crate::test_support::observation_for(
                "summary-feedback",
                event,
                "summaryneedle delayed capture",
            ),
        )?;
        let after = store.end_session_for_account(&owner.token, "summary-feedback")?;
        assert_eq!(after.memory_id, before.memory_id);
        assert!(after.source_event_ids.len() > before.source_event_ids.len());
        assert_eq!(after.access_count, before.access_count);
        assert_eq!(after.last_accessed_at, before.last_accessed_at);
        assert_eq!(after.feedback, before.feedback);
        assert_eq!(after.helpful_count, before.helpful_count);
        assert_eq!(after.stale_count, before.stale_count);
        assert_eq!(
            MemoryStore::open(&path)?
                .memories_for_account(&owner.token)?
                .into_iter()
                .find(|m| m.memory_id == summary.memory_id)
                .unwrap(),
            after
        );
    }
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn import_discards_foreign_votes_and_untrusted_counts() -> Result<(), StoreError> {
    let path = temp_store_path("import-feedback");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let saved = store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "personal", "kind": "fact", "title": "importneedle",
            "summary": "importneedle", "reason": "verified", "statement": "holds"
        }))?,
    )?;
    let mut bundle = store.export_for_account(&owner.token)?;
    bundle.sessions.clear();
    bundle.observations.clear();
    bundle.forgotten_ids.clear();
    bundle.memories = ["forged-votes", "counts-only"]
        .into_iter()
        .map(|id| {
            let mut memory = saved.clone();
            memory.memory_id = id.to_owned();
            memory.helpful_count = 999;
            memory.stale_count = 4;
            if id == "forged-votes" {
                memory.feedback = vec![
                    MemoryFeedback {
                        person_id: other.account.person_id.clone(),
                        vote: FeedbackVote::Helpful
                    };
                    3
                ];
            }
            memory
        })
        .collect();
    assert_eq!(
        store
            .import_for_account(&owner.token, bundle.clone())?
            .imported_memories,
        2
    );
    assert_eq!(
        store
            .import_for_account(&owner.token, bundle)?
            .imported_memories,
        0
    );
    let mut reopened = MemoryStore::open(&path)?;
    for memory in reopened
        .memories_for_account(&owner.token)?
        .into_iter()
        .filter(|m| m.memory_id == "forged-votes" || m.memory_id == "counts-only")
    {
        assert!(memory.feedback.is_empty(), "{}", memory.memory_id);
        assert_eq!((memory.helpful_count, memory.stale_count), (0, 0));
        let error = reopened
            .feedback_for_account(&other.token, &memory.memory_id, "helpful")
            .unwrap_err();
        assert!(error.to_string().contains("unknown memory"));
    }
    fs::remove_file(path)?;
    Ok(())
}
