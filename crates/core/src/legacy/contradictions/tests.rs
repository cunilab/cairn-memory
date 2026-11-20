use super::*;
use crate::legacy::embeddings::MemoryEmbedding;
use crate::legacy::lifecycle::MemoryKind;
use crate::legacy::memories::Memory;
use crate::legacy::recall::RecallFilter;
use crate::legacy::test_support::{join_project, save_project_memory, temp_store_path};
use crate::legacy::types::WorkContext;
use crate::legacy::{JobKind, MemoryStore};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;

fn save(store: &mut MemoryStore, token: &str, scope: &str, title: &str) -> Memory {
    store
        .save_for_account(
            token,
            serde_json::from_value(json!({
                "scope": scope,
                "kind": "fact",
                "title": title,
                "summary": title,
                "details": format!("{title} {}", crate::legacy::types::timestamp_nanos()),
                "reason": "verified",
                "statement": title
            }))
            .unwrap(),
        )
        .unwrap()
}

fn unit_pair(cosine: f32) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; 384];
    let mut right = vec![0.0; 384];
    left[0] = 1.0;
    right[0] = cosine;
    right[1] = (1.0 - cosine * cosine).sqrt();
    (left, right)
}

fn embed(store: &mut MemoryStore, memory: &Memory, vector: Vec<f32>) {
    store
        .put_embedding(MemoryEmbedding::new(memory, vector).unwrap())
        .unwrap();
}

fn unit_vector() -> Vec<f32> {
    let mut vector = vec![0.0; 384];
    vector[0] = 1.0;
    vector
}

#[test]
fn similarity_scan_keeps_sixty_current_memories_and_skips_stale_and_warm_rows() {
    let path = temp_store_path("similarity-bound");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let vector = unit_vector();
    let mut stale = Vec::new();
    let mut current = Vec::new();
    for index in 0..60 {
        let memory = save(
            &mut store,
            &owner.token,
            "personal",
            &format!("bare-{index}"),
        );
        store
            .state
            .memories
            .iter_mut()
            .find(|item| item.memory_id == memory.memory_id)
            .unwrap()
            .access_count = 0;
        let memory = save(
            &mut store,
            &owner.token,
            "personal",
            &format!("stale-{index}"),
        );
        embed(&mut store, &memory, vector.clone());
        let stored = store
            .state
            .memories
            .iter_mut()
            .find(|item| item.memory_id == memory.memory_id)
            .unwrap();
        stored.fields.title = format!("rewritten-{index}");
        stored.access_count = 1;
        stale.push(stored.memory_id.clone());
        let memory = save(
            &mut store,
            &owner.token,
            "personal",
            &format!("cold-{index}"),
        );
        embed(&mut store, &memory, vector.clone());
        store
            .state
            .memories
            .iter_mut()
            .find(|item| item.memory_id == memory.memory_id)
            .unwrap()
            .access_count = 2;
        current.push(memory.memory_id);
    }
    let warm = save(&mut store, &owner.token, "personal", "warm-extra");
    embed(&mut store, &warm, vector.clone());
    store
        .state
        .memories
        .iter_mut()
        .find(|item| item.memory_id == warm.memory_id)
        .unwrap()
        .access_count = 3;
    let capture = save(&mut store, &owner.token, "personal", "capture-row");
    embed(&mut store, &capture, vector);
    let stored = store
        .state
        .memories
        .iter_mut()
        .find(|item| item.memory_id == capture.memory_id)
        .unwrap();
    stored.kind = MemoryKind::Capture;
    stored.access_count = 0;
    let now = 1_000_u64;
    store.ensure_similarity_job(now).unwrap();
    let claim = store
        .poll_jobs(JobKind::Similarity, 1, now)
        .unwrap()
        .remove(0);
    assert_eq!(
        store.finish_similarity_scan(&claim, now).unwrap(),
        SIMILARITY_REVIEW_CAP
    );
    let reviews = store.similarity_reviews_for_account(&owner.token).unwrap();
    assert_eq!(reviews.len(), SIMILARITY_REVIEW_CAP);
    let current_ids = current.iter().map(String::as_str).collect::<HashSet<_>>();
    assert!(reviews.iter().all(|review| {
        current_ids.contains(review.left_id.as_str())
            && current_ids.contains(review.right_id.as_str())
    }));
    assert!(reviews.iter().all(|review| {
        review.left_id != warm.memory_id
            && review.right_id != warm.memory_id
            && review.left_id != capture.memory_id
            && review.right_id != capture.memory_id
            && !stale
                .iter()
                .any(|id| id == &review.left_id || id == &review.right_id)
    }));
    fs::remove_file(path).unwrap();
}

#[test]
fn contradiction_factor_floors_and_band_edges_are_exact() {
    assert_eq!(contradiction_factor(0), 1.0);
    assert_eq!(contradiction_factor(1), 0.5);
    assert_eq!(contradiction_factor(3), 0.25);
    assert_eq!(contradiction_factor(4), CONTRADICTION_FLOOR);
    assert_eq!(
        similarity_band(0.40),
        Some(SimilarityBand::PossibleConflict)
    );
    assert_eq!(
        similarity_band(0.749),
        Some(SimilarityBand::PossibleConflict)
    );
    assert_eq!(similarity_band(0.75), Some(SimilarityBand::NearDuplicate));
    assert_eq!(similarity_band(1.0), Some(SimilarityBand::NearDuplicate));
    assert_eq!(similarity_band(0.399), None);
    assert_eq!(similarity_band(1.001), None);
}

#[test]
fn scan_keeps_the_coldest_sixty_and_skips_stored_pairs_and_captures() {
    let mut memories = (0..61)
        .map(|index| {
            let mut memory = save_memory(index);
            memory.access_count = index as u64;
            memory.created_at = index.to_string();
            memory
        })
        .collect::<Vec<_>>();
    memories[2].kind = MemoryKind::Capture;
    let mut vectors = HashMap::new();
    for memory in &memories {
        let mut vector = vec![0.0; 4];
        vector[0] = 1.0;
        vectors.insert(memory.memory_id.clone(), vector);
    }
    let now = 50_u64;
    let found = similarity_candidates(&memories, &vectors, &HashSet::new(), now);
    assert_eq!(found.len(), SIMILARITY_REVIEW_CAP);
    assert!(found.iter().all(|review| review.cosine == 1.0));
    assert!(
        found
            .iter()
            .all(|review| review.band == SimilarityBand::NearDuplicate)
    );
    let warm = format!("memory-{:>02}", 60);
    let capture = memories[2].memory_id.clone();
    assert!(
        found
            .iter()
            .all(|review| review.left_id != warm && review.right_id != warm)
    );
    assert!(
        found
            .iter()
            .all(|review| review.left_id != capture && review.right_id != capture)
    );
    let existing = found
        .iter()
        .map(|review| (review.left_id.clone(), review.right_id.clone()))
        .collect::<HashSet<_>>();
    let again = similarity_candidates(&memories, &vectors, &existing, now);
    assert!(
        again.iter().all(|review| {
            !existing.contains(&(review.left_id.clone(), review.right_id.clone()))
        })
    );
    let mut every = existing;
    every.extend(
        again
            .iter()
            .map(|review| (review.left_id.clone(), review.right_id.clone())),
    );
    let third = similarity_candidates(&memories[..3], &vectors, &every, now);
    assert!(third.is_empty());

    let mut separated = HashMap::new();
    let (left, right) = {
        let mut low = vec![0.0; 4];
        let mut high = vec![0.0; 4];
        low[0] = 1.0;
        high[0] = 0.5;
        high[1] = (1.0 - 0.25_f32).sqrt();
        (low, high)
    };
    separated.insert(memories[0].memory_id.clone(), left.clone());
    separated.insert(memories[1].memory_id.clone(), right);
    // Cosine in the possible-conflict range is not enough to publish a review.
    assert!(similarity_candidates(&memories[..2], &separated, &HashSet::new(), now).is_empty());
    let mut near = HashMap::new();
    let mut high = vec![0.0; 4];
    high[0] = 0.8;
    high[1] = (1.0 - 0.64_f32).sqrt();
    near.insert(memories[0].memory_id.clone(), left);
    near.insert(memories[1].memory_id.clone(), high);
    let band = similarity_candidates(&memories[..2], &near, &HashSet::new(), now);
    assert_eq!(band.len(), 1);
    assert_eq!(band[0].band, SimilarityBand::NearDuplicate);
    assert!((band[0].cosine - 0.8).abs() < 1e-5);
}

fn save_memory(index: usize) -> Memory {
    let mut memory = crate::legacy::build_saved_memory(
        serde_json::from_value(json!({
            "scope": "personal",
            "kind": "fact",
            "title": format!("t{index}"),
            "summary": "s",
            "reason": "r"
        }))
        .unwrap(),
        "alice",
    )
    .unwrap();
    memory.memory_id = format!("memory-{index:02}");
    memory
}

#[test]
fn declared_edge_shades_only_a_visible_target_and_supersede_hides_it() {
    let path = temp_store_path("contradict-shade");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .unwrap();
    let target = save(&mut store, &owner.token, "personal", "sharedtoken");
    let source = save(&mut store, &owner.token, "personal", "sharedtoken");
    let before = store
        .search_for_account(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .unwrap();
    let before_target = before
        .iter()
        .find(|hit| hit.memory_id == target.memory_id)
        .unwrap()
        .score;
    let before_source = before
        .iter()
        .find(|hit| hit.memory_id == source.memory_id)
        .unwrap()
        .score;
    let hidden = store
        .declare_contradiction(&other.token, &source.memory_id, &target.memory_id)
        .unwrap_err();
    assert!(hidden.to_string().contains("unknown memory"));
    let global = save(&mut store, &owner.token, "global", "globaltoken");
    let crossed = store
        .declare_contradiction(&owner.token, &source.memory_id, &global.memory_id)
        .unwrap_err();
    assert!(crossed.to_string().contains("same scope"));
    let declared = store
        .declare_contradiction(&owner.token, &source.memory_id, &target.memory_id)
        .unwrap();
    assert_eq!(declared.contradicts, vec![target.memory_id.clone()]);
    store
        .declare_contradiction(&owner.token, &source.memory_id, &target.memory_id)
        .unwrap();
    let after = store
        .search_for_account(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .unwrap();
    let target_score = after
        .iter()
        .find(|hit| hit.memory_id == target.memory_id)
        .unwrap()
        .score;
    let source_score = after
        .iter()
        .find(|hit| hit.memory_id == source.memory_id)
        .unwrap()
        .score;
    assert!(
        (target_score / before_target - 0.5).abs() < 1e-4,
        "{target_score}"
    );
    assert!(
        (source_score - before_source).abs() < 1e-4,
        "{source_score}"
    );
    assert!(after.iter().any(|hit| hit.memory_id == target.memory_id));
    let exported = store.export_for_account(&owner.token).unwrap();
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == source.memory_id
                && memory.contradicts == vec![target.memory_id.clone()])
    );

    let older = save(&mut store, &owner.token, "personal", "older-fact");
    let successor = store
        .save_for_account(
            &owner.token,
            serde_json::from_value(json!({
                "scope": "personal",
                "kind": "fact",
                "title": "successor-fact",
                "summary": "successor-fact",
                "reason": "verified",
                "supersedes": older.memory_id
            }))
            .unwrap(),
        )
        .unwrap();
    let blocked = store
        .supersede_memory(&owner.token, &successor.memory_id, &target.memory_id)
        .unwrap_err();
    assert!(blocked.to_string().contains("different memory"));
    store
        .supersede_memory(&owner.token, &source.memory_id, &target.memory_id)
        .unwrap();
    let recalled = store
        .search_for_account(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .unwrap();
    assert!(recalled.iter().all(|hit| hit.memory_id != target.memory_id));
    let history = store.export_for_account(&owner.token).unwrap();
    assert!(
        history
            .memories
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn project_edit_rights_gate_reviews_and_a_reader_cannot_see_a_hidden_pair() {
    let path = temp_store_path("contradict-rights");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .unwrap();
    let outsider = store
        .register_account("outsider@example.test", "correct horse battery staple")
        .unwrap();
    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .unwrap();
    join_project(&mut store, &member.token, &project.project_id).unwrap();
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    let left = save_project_memory(
        &mut store,
        &owner.token,
        &project.project_id,
        "left-note",
        context.clone(),
    )
    .unwrap();
    let right = save_project_memory(
        &mut store,
        &owner.token,
        &project.project_id,
        "right-note",
        context,
    )
    .unwrap();
    let refused = store
        .declare_contradiction(&member.token, &right.memory_id, &left.memory_id)
        .unwrap_err();
    assert!(refused.to_string().contains("author or project owner"));
    let (low, high) = unit_pair(0.8);
    embed(&mut store, &left, low);
    embed(&mut store, &right, high);
    let mut capture = left.clone();
    capture.memory_id = "capture-row".to_owned();
    capture.kind = MemoryKind::Capture;
    capture.contradicts.clear();
    capture.supersedes = None;
    store.state.memories.push(capture.clone());
    embed(&mut store, &capture, {
        let mut vector = vec![0.0; 384];
        vector[0] = 1.0;
        vector
    });
    let now = 1_000_u64;
    store.ensure_similarity_job(now).unwrap();
    let claim = store
        .poll_jobs(JobKind::Similarity, 1, now)
        .unwrap()
        .remove(0);
    assert_eq!(store.finish_similarity_scan(&claim, now).unwrap(), 1);
    assert!(
        store
            .poll_jobs(
                JobKind::Similarity,
                1,
                now + SIMILARITY_INTERVAL_SECONDS - 1
            )
            .unwrap()
            .is_empty()
    );
    let reviews = store.similarity_reviews_for_account(&owner.token).unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].band, SimilarityBand::NearDuplicate);
    assert!(
        store
            .similarity_reviews_for_account(&outsider.token)
            .unwrap()
            .is_empty()
    );
    let leaked = store
        .dismiss_review(&outsider.token, &reviews[0].review_id)
        .unwrap_err();
    assert!(leaked.to_string().contains("unknown review"));
    let member_refused = store
        .dismiss_review(&member.token, &reviews[0].review_id)
        .unwrap_err();
    assert!(
        member_refused
            .to_string()
            .contains("author or project owner")
    );
    let contradicted = store
        .contradict_review(&owner.token, &reviews[0].review_id, &left.memory_id)
        .unwrap();
    assert!(contradicted.contradicts.contains(&left.memory_id));
    assert!(
        store
            .similarity_reviews_for_account(&owner.token)
            .unwrap()
            .is_empty()
    );
    fs::remove_file(path).unwrap();
}

fn project_fact(store: &mut MemoryStore, token: &str, project_id: &str, title: &str) -> Memory {
    store
        .save_for_account(
            token,
            serde_json::from_value(json!({
                "scope": "project",
                "kind": "fact",
                "title": title,
                "summary": title,
                "reason": "verified",
                "statement": title,
                "context": {"project_id": project_id}
            }))
            .unwrap(),
        )
        .unwrap()
}

fn session_note(
    session: &str,
    event: &str,
    content: &str,
) -> crate::legacy::capture::ObservationInput {
    crate::legacy::capture::ObservationInput {
        event_id: event.to_owned(),
        session_id: session.to_owned(),
        content: content.to_owned(),
        role: "user".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: Default::default(),
        context: WorkContext::default(),
    }
}

fn same_direction() -> Vec<f32> {
    let mut vector = vec![0.0; 384];
    vector[0] = 1.0;
    vector
}

#[test]
fn dismiss_requires_edit_rights_on_both_memories() {
    let path = temp_store_path("dismiss-both");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let left_author = store
        .register_account("left@example.test", "correct horse battery staple")
        .unwrap();
    let right_author = store
        .register_account("right@example.test", "correct horse battery staple")
        .unwrap();
    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .unwrap();
    join_project(&mut store, &left_author.token, &project.project_id).unwrap();
    join_project(&mut store, &right_author.token, &project.project_id).unwrap();
    let left = project_fact(
        &mut store,
        &left_author.token,
        &project.project_id,
        "dismiss-left",
    );
    let right = project_fact(
        &mut store,
        &right_author.token,
        &project.project_id,
        "dismiss-right",
    );
    embed(&mut store, &left, same_direction());
    embed(&mut store, &right, same_direction());
    store.ensure_similarity_job(1_000).unwrap();
    let claim = store
        .poll_jobs(JobKind::Similarity, 1, 1_000)
        .unwrap()
        .remove(0);
    assert_eq!(store.finish_similarity_scan(&claim, 1_000).unwrap(), 1);
    let review = store
        .similarity_reviews_for_account(&left_author.token)
        .unwrap()
        .into_iter()
        .find(|review| review.left_id == left.memory_id || review.right_id == left.memory_id)
        .unwrap();
    let declare = store
        .declare_contradiction(&left_author.token, &left.memory_id, &right.memory_id)
        .unwrap_err();
    assert!(declare.to_string().contains("author or project owner"));
    let dismissed = store
        .dismiss_review(&left_author.token, &review.review_id)
        .unwrap_err();
    assert!(dismissed.to_string().contains("author or project owner"));
    store
        .dismiss_review(&owner.token, &review.review_id)
        .unwrap();
    assert!(
        store
            .similarity_reviews_for_account(&owner.token)
            .unwrap()
            .is_empty()
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn cross_author_project_supersede_hides_the_target() {
    let path = temp_store_path("cross-supersede");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .unwrap();
    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .unwrap();
    join_project(&mut store, &member.token, &project.project_id).unwrap();
    let target = project_fact(
        &mut store,
        &owner.token,
        &project.project_id,
        "crosstoken target",
    );
    let source = project_fact(
        &mut store,
        &member.token,
        &project.project_id,
        "crosstoken source",
    );
    store
        .supersede_memory(&owner.token, &source.memory_id, &target.memory_id)
        .unwrap();
    assert!(
        !store
            .memories_for_account(&owner.token)
            .unwrap()
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
    let found = store
        .search_for_account(&owner.token, "crosstoken", 10, &RecallFilter::default())
        .unwrap();
    assert!(found.iter().all(|hit| hit.memory_id != target.memory_id));
    fs::remove_file(path).unwrap();
}

#[test]
fn rederived_summary_keeps_an_explicit_supersede() {
    let path = temp_store_path("rederive-supersede");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let target = save(&mut store, &owner.token, "personal", "summary-target");
    store
        .start_session_for_account(&owner.token, "reuse", WorkContext::default())
        .unwrap();
    store
        .capture_for_account(
            &owner.token,
            session_note(
                "reuse",
                "first",
                "Decision: use first because tests passed.",
            ),
        )
        .unwrap();
    let summary = store
        .end_session_for_account(&owner.token, "reuse")
        .unwrap();
    store
        .supersede_memory(&owner.token, &summary.memory_id, &target.memory_id)
        .unwrap();
    assert!(
        !store
            .memories_for_account(&owner.token)
            .unwrap()
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
    store
        .capture_for_account(
            &owner.token,
            session_note(
                "reuse",
                "second",
                "Decision: use second because tests passed.",
            ),
        )
        .unwrap();
    let derived = store
        .end_session_for_account(&owner.token, "reuse")
        .unwrap();
    assert_eq!(
        derived.supersedes.as_deref(),
        Some(target.memory_id.as_str())
    );
    assert!(
        !store
            .memories_for_account(&owner.token)
            .unwrap()
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn distilled_notes_do_not_inherit_declared_contradictions() {
    let path = temp_store_path("inherit-contradicts");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let target = save(&mut store, &owner.token, "personal", "shade-target");
    store
        .start_session_for_account(&owner.token, "inherit", WorkContext::default())
        .unwrap();
    store
        .capture_for_account(
            &owner.token,
            session_note("inherit", "first", "Fact: unrelated first"),
        )
        .unwrap();
    let summary = store
        .end_session_for_account(&owner.token, "inherit")
        .unwrap();
    store
        .declare_contradiction(&owner.token, &summary.memory_id, &target.memory_id)
        .unwrap();
    assert_eq!(
        contradiction_counts(&store.memories_for_account(&owner.token).unwrap())[&target.memory_id],
        1
    );
    store
        .capture_for_account(
            &owner.token,
            session_note("inherit", "second", "Fact: unrelated second"),
        )
        .unwrap();
    store
        .end_session_for_account(&owner.token, "inherit")
        .unwrap();
    let memories = store.memories_for_account(&owner.token).unwrap();
    assert_eq!(contradiction_counts(&memories)[&target.memory_id], 1);
    assert!(memories.iter().all(|memory| {
        memory.kind != MemoryKind::Fact || !memory.contradicts.contains(&target.memory_id)
    }));
    let successor = save(&mut store, &owner.token, "personal", "summary-successor");
    store
        .supersede_memory(&owner.token, &successor.memory_id, &summary.memory_id)
        .unwrap();
    let remaining = contradiction_counts(&store.memories_for_account(&owner.token).unwrap());
    assert_eq!(remaining.get(&target.memory_id).copied().unwrap_or(0), 0);
    fs::remove_file(path).unwrap();
}

#[test]
fn expired_similarity_claim_publishes_nothing() {
    let path = temp_store_path("expired-scan");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let left = save(&mut store, &owner.token, "global", "scan-a");
    let right = save(&mut store, &owner.token, "global", "scan-b");
    embed(&mut store, &left, same_direction());
    embed(&mut store, &right, same_direction());
    store.ensure_similarity_job(1_000).unwrap();
    let claim = store
        .poll_jobs(JobKind::Similarity, 1, 1_000)
        .unwrap()
        .remove(0);
    assert!(
        store
            .poll_jobs(JobKind::Similarity, 1, 1_121)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.finish_similarity_scan(&claim, 1_121).unwrap(), 0);
    assert!(
        store
            .similarity_reviews_for_account(&owner.token)
            .unwrap()
            .is_empty()
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn identical_normalized_embeddings_stay_in_the_similarity_band() {
    let mut state = 17_u32;
    for _case in 0..100 {
        let mut vector = (0..384)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
            })
            .collect::<Vec<_>>();
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        for value in &mut vector {
            *value /= norm;
        }
        crate::legacy::validate_vector(&vector).unwrap();
        let score = cosine(&vector, &vector).unwrap();
        assert!(
            similarity_band(score).is_some(),
            "identical embeddings left the review band at {score}"
        );
        let mut left = crate::legacy::build_saved_memory(
            serde_json::from_value(json!({
                "scope": "personal",
                "kind": "fact",
                "title": "same",
                "summary": "same",
                "reason": "verified",
                "statement": "same"
            }))
            .unwrap(),
            "author",
        )
        .unwrap();
        let mut right = left.clone();
        left.memory_id = "left".to_owned();
        right.memory_id = "right".to_owned();
        let vectors = HashMap::from([
            ("left".to_owned(), vector.clone()),
            ("right".to_owned(), vector),
        ]);
        let found = similarity_candidates(&[left, right], &vectors, &HashSet::new(), 1_000);
        assert_eq!(found.len(), 1, "{score}");
    }
}
