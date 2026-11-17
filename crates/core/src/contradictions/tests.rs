use super::*;
use crate::embeddings::MemoryEmbedding;
use crate::lifecycle::MemoryKind;
use crate::memories::Memory;
use crate::recall::RecallFilter;
use crate::test_support::{join_project, save_project_memory, temp_store_path};
use crate::types::WorkContext;
use crate::{JobKind, MemoryStore};
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
    separated.insert(memories[0].memory_id.clone(), left);
    separated.insert(memories[1].memory_id.clone(), right);
    let band = similarity_candidates(&memories[..2], &separated, &HashSet::new(), now);
    assert_eq!(band.len(), 1);
    assert_eq!(band[0].band, SimilarityBand::PossibleConflict);
    assert!((band[0].cosine - 0.5).abs() < 1e-5);
}

fn save_memory(index: usize) -> Memory {
    let mut memory = crate::build_saved_memory(
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
    let (low, high) = unit_pair(0.5);
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
    assert_eq!(reviews[0].band, SimilarityBand::PossibleConflict);
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
