use crate::capture::ObservationInput;
use crate::lifecycle::{LifecycleInput, MemoryKind, MemoryScope};
use crate::recall::{ContextPack, RecallFilter};
use crate::store::MemoryStore;
use crate::test_support::{
    context, empty_capture, observation_for, remember_owned, temp_store_path,
};
use crate::types::{StoreError, WorkContext, now_seconds};
use serde_json::json;
use std::collections::HashSet;
use std::fs;

#[test]
fn why_changed_reports_reason_relationships_staleness_and_contradictions() -> Result<(), StoreError>
{
    let path = temp_store_path("why-changed");
    let mut store = MemoryStore::open(&path)?;
    let project_context = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        files: vec!["src/cache.rs".to_owned()],
        symbols: vec!["Cache::get".to_owned()],
        commands: vec!["cargo test cache".to_owned()],
        tests: vec!["cache_reads_are_consistent".to_owned()],
        services: vec!["cache".to_owned()],
        ..WorkContext::default()
    };
    let stale_context = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some("feat/cache".to_owned()),
        worktree: Some("wt-cache".to_owned()),
        commit: Some("abc1234".to_owned()),
        ..WorkContext::default()
    };
    let current_context = WorkContext {
        commit: Some("def5678".to_owned()),
        ..stale_context.clone()
    };
    let first = store.remember(
        "The project uses Redis as the default cache because reads are fast.",
        Vec::new(),
        project_context,
    )?;
    let second = store.remember(
        "The project does not use Redis as the default cache because consistency is preferred.",
        Vec::new(),
        WorkContext {
            repository: Some("cairn-demo".to_owned()),
            ..WorkContext::default()
        },
    )?;
    let stale = store.remember_with_lifecycle(
        "The cache experiment remains valid because the old benchmark passed.",
        Vec::new(),
        stale_context,
        LifecycleInput {
            freshness: Some(40),
            ..LifecycleInput::default()
        },
    )?;

    let filter = RecallFilter {
        session_id: None,
        context: Some(current_context),
        ..RecallFilter::default()
    };
    let pack = store.why_changed_context("Redis default cache", 10, &filter)?;
    let first_result = pack
        .results
        .iter()
        .find(|result| result.memory_id == first.memory_id)
        .unwrap();
    let second_result = pack
        .results
        .iter()
        .find(|result| result.memory_id == second.memory_id)
        .unwrap();
    let stale_result = pack
        .results
        .iter()
        .find(|result| result.memory_id == stale.memory_id)
        .unwrap();
    assert_eq!(
        first_result.decision_reason.as_deref(),
        Some("reads are fast")
    );
    assert!(first_result.relationships.iter().any(|relationship| {
        relationship.relation == "file_contains_symbol"
            && relationship.from == "src/cache.rs"
            && relationship.to == "Cache::get"
    }));
    assert!(first_result.relationships.iter().any(|relationship| {
        relationship.relation == "symbol_verified_by_test"
            && relationship.to == "cache_reads_are_consistent"
    }));
    assert!(second_result.contradictory);
    assert!(second_result.contradiction_ids.contains(&first.memory_id));
    assert!(stale_result.stale);
    assert_eq!(
        stale_result.stale_reason.as_deref(),
        Some("memory freshness is below 50")
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn account_search_limits_owned_matches_and_hides_foreign_contradiction_ids()
-> Result<(), StoreError> {
    let path = temp_store_path("account-search-limit");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let explicit = |store: &mut MemoryStore, token: Option<&str>, content: &str| match token {
        Some(token) => store.remember_for_account(
            token,
            content,
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        ),
        None => store.remember(content, Vec::new(), WorkContext::default()),
    };

    let foreign = explicit(
        &mut store,
        Some(&member.token),
        "ranktoken ranktoken ranktoken foreignmarker",
    )?;
    let _unowned = explicit(&mut store, None, "ranktoken ranktoken unownedmarker")?;
    std::thread::sleep(std::time::Duration::from_millis(2));
    let owned_early = explicit(&mut store, Some(&owner.token), "ranktoken ownedearly")?;
    std::thread::sleep(std::time::Duration::from_millis(2));
    let owned_late = explicit(&mut store, Some(&owner.token), "ranktoken ownedlate")?;
    assert!(owned_early.memory_id < owned_late.memory_id);

    let limit_one = store.search_for_account(&owner.token, "ranktoken", 1, &filter)?;
    assert_eq!(limit_one.len(), 1);
    assert_eq!(limit_one[0].memory_id, owned_early.memory_id);
    assert!((limit_one[0].score - 2.0 / 61.0).abs() < 1e-7);
    assert!(!limit_one[0].content.contains("foreignmarker"));
    assert!(!limit_one[0].content.contains("unownedmarker"));

    let limit_two = store.search_for_account(&owner.token, "ranktoken", 2, &filter)?;
    assert_eq!(
        limit_two
            .iter()
            .map(|result| result.memory_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            owned_early.memory_id.as_str(),
            owned_late.memory_id.as_str()
        ]
    );
    let again = store.search_for_account(&owner.token, "ranktoken", 2, &filter)?;
    assert_eq!(again, limit_two);
    assert!(
        store
            .search_for_account(&owner.token, "ranktoken", 0, &filter)?
            .is_empty()
    );

    let global = store.search_with_filter("ranktoken", 1, &filter)?;
    assert_eq!(global.len(), 1);
    assert_eq!(global[0].memory_id, foreign.memory_id);

    std::thread::sleep(std::time::Duration::from_millis(2));
    let tie_foreign = explicit(&mut store, Some(&member.token), "tiealpha tieforeign")?;
    std::thread::sleep(std::time::Duration::from_millis(2));
    let tie_owned = explicit(&mut store, Some(&owner.token), "tiealpha tieowned")?;
    assert!(tie_foreign.memory_id < tie_owned.memory_id);
    let tie = store.search_for_account(&owner.token, "tiealpha", 1, &filter)?;
    assert_eq!(tie.len(), 1);
    assert_eq!(tie[0].memory_id, tie_owned.memory_id);
    assert!(!tie[0].content.contains("tieforeign"));

    let signal_owned = explicit(
        &mut store,
        Some(&owner.token),
        "signalbeta lattice does not apply",
    )?;
    let signal_foreign = explicit(
        &mut store,
        Some(&member.token),
        "signalbeta lattice does apply",
    )?;
    let _signal_unowned = explicit(&mut store, None, "signalbeta lattice does apply")?;
    let signal = store.search_for_account(&owner.token, "signalbeta", 5, &filter)?;
    assert_eq!(signal.len(), 1);
    assert_eq!(signal[0].memory_id, signal_owned.memory_id);
    assert_eq!(signal[0].content, "signalbeta lattice does not apply");
    assert!(!signal[0].contradictory);
    assert!(signal[0].contradiction_ids.is_empty());
    assert!(signal[0].decision_reason.is_none());
    assert!(signal[0].relationships.is_empty());
    let unscoped_signal = store.search_with_filter("signalbeta", 5, &filter)?;
    let unscoped_owned = unscoped_signal
        .iter()
        .find(|result| result.memory_id == signal_owned.memory_id)
        .unwrap();
    assert!(unscoped_owned.contradictory);
    assert!(
        unscoped_owned
            .contradiction_ids
            .contains(&signal_foreign.memory_id)
    );

    std::thread::sleep(std::time::Duration::from_millis(2));
    let quota_neg = explicit(
        &mut store,
        Some(&owner.token),
        "quotadelta widget does not apply",
    )?;
    std::thread::sleep(std::time::Duration::from_millis(2));
    let quota_pos = explicit(
        &mut store,
        Some(&owner.token),
        "quotadelta widget does apply",
    )?;
    let quota_foreign = explicit(
        &mut store,
        Some(&member.token),
        "quotadelta widget does apply",
    )?;
    assert!(quota_neg.memory_id < quota_pos.memory_id);
    // Keep ownership/limit assertions independent of second-boundary timing.
    for memory in &mut store.state.memories {
        if memory.memory_id == quota_neg.memory_id || memory.memory_id == quota_pos.memory_id {
            memory.created_at = "100".to_owned();
        }
    }
    let quota = store.search_for_account(&owner.token, "quotadelta", 5, &filter)?;
    assert_eq!(quota.len(), 2);
    let neg = quota
        .iter()
        .find(|result| result.memory_id == quota_neg.memory_id)
        .unwrap();
    let pos = quota
        .iter()
        .find(|result| result.memory_id == quota_pos.memory_id)
        .unwrap();
    assert!(neg.contradictory);
    assert_eq!(neg.content, "quotadelta widget does not apply");
    assert_eq!(neg.contradiction_ids, vec![quota_pos.memory_id.clone()]);
    assert!(pos.contradictory);
    assert_eq!(pos.content, "quotadelta widget does apply");
    assert_eq!(pos.contradiction_ids, vec![quota_neg.memory_id.clone()]);

    let context = store.context_for_account(&owner.token, "quotadelta", 1, &filter)?;
    assert_eq!(context.results.len(), 1);
    assert_eq!(context.results[0].memory_id, quota_neg.memory_id);
    assert_eq!(
        context.results[0].contradiction_ids,
        vec![quota_pos.memory_id.clone()]
    );
    assert!(context.results[0].contradictory);
    assert_eq!(context.omitted_count, 1);
    assert_eq!(
        context.omitted_memory_ids,
        vec![quota_pos.memory_id.clone()]
    );
    assert!(
        !context.results[0]
            .contradiction_ids
            .contains(&quota_foreign.memory_id)
    );

    let why = store.why_changed_for_account(&owner.token, "quotadelta", 5, &filter)?;
    assert_eq!(why.results.len(), 2);
    assert!(why.omitted_memory_ids.is_empty());
    assert!(why.results.iter().all(|result| {
        (result.memory_id == quota_neg.memory_id || result.memory_id == quota_pos.memory_id)
            && result.contradictory
            && !result.contradiction_ids.contains(&quota_foreign.memory_id)
            && result.content.contains("quotadelta widget")
    }));

    let rank_context = store.context_for_account(&owner.token, "ranktoken", 1, &filter)?;
    assert_eq!(rank_context.results[0].memory_id, owned_early.memory_id);
    assert_eq!(
        rank_context.omitted_memory_ids,
        vec![owned_late.memory_id.clone()]
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn account_resume_and_unresolved_hide_foreign_contradiction_ids() -> Result<(), StoreError> {
    let path = temp_store_path("account-resume-contradiction");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let branch = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some("main".to_owned()),
        ..WorkContext::default()
    };
    let capture = |store: &mut MemoryStore,
                   token: &str,
                   session_id: &str,
                   event_id: &str,
                   content: &str,
                   context: WorkContext| {
        store.capture_for_account(
            token,
            ObservationInput {
                context,
                ..empty_capture(session_id, event_id, content)
            },
        )
    };

    store.start_session_for_account(&owner.token, "handoff-topic", WorkContext::default())?;
    store.start_session_for_account(&member.token, "handoff-topic", WorkContext::default())?;
    capture(
        &mut store,
        &owner.token,
        "handoff-topic",
        "owner-handoff-evt",
        "quotadelta widget does not apply remaining",
        WorkContext::default(),
    )?;
    capture(
        &mut store,
        &member.token,
        "handoff-topic",
        "member-handoff-evt",
        "quotadelta widget does apply foreignmarker remaining",
        WorkContext::default(),
    )?;
    let owner_handoff = store.end_session_for_account(&owner.token, "handoff-topic")?;
    let member_handoff = store.end_session_for_account(&member.token, "handoff-topic")?;
    assert_eq!(owner_handoff.scope, MemoryScope::Personal);
    assert_eq!(member_handoff.scope, MemoryScope::Personal);
    assert_eq!(owner_handoff.session_id, member_handoff.session_id);

    store.start_session_for_account(&owner.token, "branch-neg", branch.clone())?;
    store.start_session_for_account(&owner.token, "branch-pos", branch.clone())?;
    store.start_session_for_account(&member.token, "branch-neg", branch.clone())?;
    capture(
        &mut store,
        &owner.token,
        "branch-neg",
        "owner-neg-evt",
        "latticebeta signal does not apply remaining",
        branch.clone(),
    )?;
    capture(
        &mut store,
        &owner.token,
        "branch-pos",
        "owner-pos-evt",
        "latticebeta signal does apply remaining",
        branch.clone(),
    )?;
    capture(
        &mut store,
        &member.token,
        "branch-neg",
        "member-branch-evt",
        "latticebeta signal does apply foreignmarker remaining",
        branch.clone(),
    )?;
    let owner_neg = store.end_session_for_account(&owner.token, "branch-neg")?;
    let owner_pos = store.end_session_for_account(&owner.token, "branch-pos")?;
    let member_branch = store.end_session_for_account(&member.token, "branch-neg")?;
    assert_eq!(owner_neg.scope, MemoryScope::Personal);
    assert_eq!(owner_pos.scope, MemoryScope::Personal);

    let hides_foreign = |pack: &ContextPack| {
        let json = serde_json::to_string(pack).expect("pack json");
        assert!(!json.contains(&member.account.person_id), "{json}");
        assert!(!json.contains(&member_handoff.memory_id), "{json}");
        assert!(!json.contains(&member_branch.memory_id), "{json}");
        assert!(!json.contains("member-handoff-evt"), "{json}");
        assert!(!json.contains("member-branch-evt"), "{json}");
        assert!(!json.contains("foreignmarker"), "{json}");
    };

    let resume = store.resume_for_account(&owner.token, 5, &filter)?;
    let handoff = resume
        .results
        .iter()
        .find(|result| result.memory_id == owner_handoff.memory_id)
        .unwrap();
    assert!(!handoff.contradictory);
    assert!(handoff.contradiction_ids.is_empty());
    assert!(handoff.content.contains("does not apply"));
    let pos = resume
        .results
        .iter()
        .find(|result| result.memory_id == owner_pos.memory_id)
        .unwrap();
    let neg = resume
        .results
        .iter()
        .find(|result| result.memory_id == owner_neg.memory_id)
        .unwrap();
    assert!(pos.contradictory);
    assert_eq!(
        pos.contradiction_ids,
        vec![
            format!("handoff-{}", owner_neg.memory_id),
            owner_neg.memory_id.clone()
        ]
    );
    assert!(neg.contradictory);
    assert_eq!(
        neg.contradiction_ids,
        vec![
            format!("handoff-{}", owner_pos.memory_id),
            owner_pos.memory_id.clone()
        ]
    );
    hides_foreign(&resume);

    let packed = store.resume_for_account(&owner.token, 1, &filter)?;
    assert_eq!(packed.results.len(), 1);
    assert_eq!(packed.results[0].memory_id, owner_pos.memory_id);
    assert_eq!(
        packed.results[0].contradiction_ids,
        vec![
            format!("handoff-{}", owner_neg.memory_id),
            owner_neg.memory_id.clone()
        ]
    );
    assert_eq!(
        packed.omitted_memory_ids,
        vec![owner_neg.memory_id.clone(), owner_handoff.memory_id.clone()]
    );
    hides_foreign(&packed);

    let zero = store.resume_for_account(&owner.token, 0, &filter)?;
    assert!(zero.results.is_empty());
    assert_eq!(zero.omitted_count, 3);
    assert_eq!(
        zero.omitted_memory_ids,
        vec![
            owner_pos.memory_id.clone(),
            owner_neg.memory_id.clone(),
            owner_handoff.memory_id.clone()
        ]
    );
    hides_foreign(&zero);

    let unresolved = store.unresolved_for_account(&owner.token, 5, &filter)?;
    let unresolved_handoff = unresolved
        .results
        .iter()
        .find(|result| result.memory_id == owner_handoff.memory_id)
        .unwrap();
    assert!(!unresolved_handoff.contradictory);
    assert!(unresolved_handoff.contradiction_ids.is_empty());
    let unresolved_neg = unresolved
        .results
        .iter()
        .find(|result| result.memory_id == owner_neg.memory_id)
        .unwrap();
    assert!(unresolved_neg.contradictory);
    assert_eq!(
        unresolved_neg.contradiction_ids,
        vec![
            format!("handoff-{}", owner_pos.memory_id),
            owner_pos.memory_id.clone()
        ]
    );
    hides_foreign(&unresolved);
    let unresolved_zero = store.unresolved_for_account(&owner.token, 0, &filter)?;
    assert!(unresolved_zero.results.is_empty());
    hides_foreign(&unresolved_zero);

    let hits = store.search_for_account(&owner.token, "quotadelta", 5, &filter)?;
    assert_eq!(hits.len(), 2, "session note and separate handoff");
    assert!(hits.iter().any(|m| m.memory_id == owner_handoff.memory_id));
    assert!(!hits[0].contradictory);
    assert!(hits[0].contradiction_ids.is_empty());
    let context = store.context_for_account(&owner.token, "quotadelta", 1, &filter)?;
    assert_eq!(context.results[0].memory_id, hits[0].memory_id);
    assert!(context.results[0].contradiction_ids.is_empty());
    hides_foreign(&context);
    let why = store.why_changed_for_account(&owner.token, "quotadelta", 1, &filter)?;
    assert!(hits.iter().any(|m| m.memory_id == why.results[0].memory_id));
    assert!(why.results[0].contradiction_ids.is_empty());
    hides_foreign(&why);

    let unscoped = store.resume_context(20, &filter)?;
    let unscoped_owner = unscoped
        .results
        .iter()
        .find(|result| result.memory_id == owner_handoff.memory_id)
        .unwrap();
    assert!(unscoped_owner.contradictory);
    assert!(
        unscoped_owner
            .contradiction_ids
            .contains(&member_handoff.memory_id)
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn contradiction_targets_follow_recall_eligibility() -> Result<(), StoreError> {
    let path = temp_store_path("contradiction-eligibility");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let branch = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some("main".to_owned()),
        ..WorkContext::default()
    };
    let other_branch = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some("topic/other".to_owned()),
        ..WorkContext::default()
    };
    let owned = |store: &mut MemoryStore, content: &str, context: WorkContext| {
        store.remember_for_account(
            &owner.token,
            content,
            Vec::new(),
            context,
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )
    };
    let quiet = |store: &mut MemoryStore, query: &str| -> Result<(), StoreError> {
        for results in [
            store.search_for_account(&owner.token, query, 5, &filter)?,
            store
                .context_for_account(&owner.token, query, 5, &filter)?
                .results,
            store
                .why_changed_for_account(&owner.token, query, 5, &filter)?
                .results,
        ] {
            assert_eq!(
                results
                    .iter()
                    .filter(|m| !m.memory_id.starts_with("handoff-"))
                    .count(),
                1,
                "{query}"
            );
            for result in results {
                assert!(!result.contradictory, "{query}");
                assert!(result.contradiction_ids.is_empty(), "{query}");
            }
        }
        Ok(())
    };

    let neg = owned(
        &mut store,
        "alphaquota widget does not apply",
        branch.clone(),
    )?;
    let pos = owned(&mut store, "alphaquota widget does apply", branch.clone())?;
    assert!(neg.memory_id < pos.memory_id);
    let other = owned(
        &mut store,
        "alphaquota widget does apply",
        other_branch.clone(),
    )?;
    let foreign = store.remember_for_account(
        &member.token,
        "alphaquota widget does apply foreignmarker",
        Vec::new(),
        branch.clone(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;

    let limited = store.search_for_account(&owner.token, "alphaquota", 1, &filter)?;
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].memory_id, neg.memory_id);
    assert_eq!(limited[0].content, "alphaquota widget does not apply");
    assert!(limited[0].contradictory);
    assert_eq!(limited[0].contradiction_ids, vec![pos.memory_id.clone()]);
    let packed = store.context_for_account(&owner.token, "alphaquota", 1, &filter)?;
    assert_eq!(packed.results[0].memory_id, neg.memory_id);
    assert_eq!(
        packed.results[0].contradiction_ids,
        vec![pos.memory_id.clone()]
    );
    assert!(packed.results[0].contradictory);
    assert_eq!(packed.omitted_count, 2);
    assert!(packed.omitted_memory_ids.contains(&pos.memory_id));
    assert!(packed.omitted_memory_ids.contains(&other.memory_id));
    assert!(!packed.omitted_memory_ids.contains(&foreign.memory_id));
    let why = store.why_changed_for_account(&owner.token, "alphaquota", 5, &filter)?;
    let why_neg = why
        .results
        .iter()
        .find(|result| result.memory_id == neg.memory_id)
        .unwrap();
    let why_pos = why
        .results
        .iter()
        .find(|result| result.memory_id == pos.memory_id)
        .unwrap();
    assert_eq!(why_neg.contradiction_ids, vec![pos.memory_id.clone()]);
    assert_eq!(why_pos.contradiction_ids, vec![neg.memory_id.clone()]);
    assert!(why.results.iter().all(|result| {
        !result.contradiction_ids.contains(&other.memory_id)
            && !result.contradiction_ids.contains(&foreign.memory_id)
            && !result.content.contains("foreignmarker")
    }));
    let unscoped = store.search_with_filter("alphaquota", 10, &filter)?;
    let unscoped_neg = unscoped
        .iter()
        .find(|result| result.memory_id == neg.memory_id)
        .unwrap();
    assert_eq!(
        unscoped_neg.contradiction_ids,
        vec![pos.memory_id.clone(), foreign.memory_id.clone()]
    );
    assert!(!unscoped_neg.contradiction_ids.contains(&other.memory_id));

    let predecessor = owned(
        &mut store,
        "corrquota cache does apply",
        WorkContext::default(),
    )?;
    let peer = owned(
        &mut store,
        "corrquota cache does not apply",
        WorkContext::default(),
    )?;
    let correction = store.correct_for_account(
        &owner.token,
        &predecessor.memory_id,
        "corrquota cache does apply",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    assert_eq!(
        correction.supersedes.as_deref(),
        Some(predecessor.memory_id.as_str())
    );
    let while_live = store.search_for_account(&owner.token, "corrquota", 5, &filter)?;
    assert!(
        while_live
            .iter()
            .all(|result| result.memory_id != predecessor.memory_id)
    );
    let live_peer = while_live
        .iter()
        .find(|result| result.memory_id == peer.memory_id)
        .unwrap();
    assert_eq!(
        live_peer.contradiction_ids,
        vec![correction.memory_id.clone()]
    );
    store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.memory_id == correction.memory_id)
        .unwrap()
        .lifecycle
        .expires_at = Some(now_seconds().saturating_sub(1).to_string());
    assert!(!store.state.forgotten_ids.contains(&correction.memory_id));
    let after_expiry = store.search_for_account(&owner.token, "corrquota", 5, &filter)?;
    assert!(
        after_expiry
            .iter()
            .all(|result| result.memory_id != correction.memory_id)
    );
    let restored = after_expiry
        .iter()
        .find(|result| result.memory_id == predecessor.memory_id)
        .unwrap();
    let restored_peer = after_expiry
        .iter()
        .find(|result| result.memory_id == peer.memory_id)
        .unwrap();
    assert_eq!(restored.contradiction_ids, vec![peer.memory_id.clone()]);
    assert_eq!(
        restored_peer.contradiction_ids,
        vec![predecessor.memory_id.clone()]
    );
    let why_restored = store.why_changed_for_account(&owner.token, "corrquota", 5, &filter)?;
    assert!(why_restored.results.iter().all(|result| {
        !result.contradiction_ids.contains(&correction.memory_id)
            && result.memory_id != correction.memory_id
    }));

    let current = owned(&mut store, "expquota relay does apply", branch.clone())?;
    let expired = store.remember_for_account(
        &owner.token,
        "expquota relay does not apply",
        Vec::new(),
        branch.clone(),
        LifecycleInput {
            expires_at: Some(now_seconds().saturating_sub(1).to_string()),
            ..LifecycleInput::default()
        },
        MemoryKind::Explicit,
    )?;
    assert!(!store.state.forgotten_ids.contains(&expired.memory_id));
    quiet(&mut store, "expquota")?;
    let unscoped_expired = store.search_with_filter("expquota", 5, &filter)?;
    assert_eq!(unscoped_expired.len(), 1);
    assert_eq!(unscoped_expired[0].memory_id, current.memory_id);
    assert!(unscoped_expired[0].contradiction_ids.is_empty());

    store.start_session_for_account(&owner.token, "sum-live", branch.clone())?;
    store.start_session_for_account(&owner.token, "sum-dead", branch.clone())?;
    store.capture_for_account(
        &owner.token,
        ObservationInput {
            context: branch.clone(),
            ..observation_for(
                "sum-live",
                "sum-live-event",
                "sumquota sumtoken does hold unresolved",
            )
        },
    )?;
    store.capture_for_account(
        &owner.token,
        ObservationInput {
            context: branch.clone(),
            ..observation_for(
                "sum-dead",
                "sum-dead-event",
                "sumquota sumtoken does not hold unresolved",
            )
        },
    )?;
    let live_summary = store.end_session_for_account(&owner.token, "sum-live")?;
    let dead_summary = store.end_session_for_account(&owner.token, "sum-dead")?;
    let live_handoff = store.resume_for_account(&owner.token, 5, &filter)?;
    let live_row = live_handoff
        .results
        .iter()
        .find(|result| result.memory_id == live_summary.memory_id)
        .unwrap();
    assert_eq!(
        live_row.contradiction_ids,
        vec![
            format!("handoff-{}", dead_summary.memory_id),
            dead_summary.memory_id.clone()
        ]
    );
    store.forget_for_account(&owner.token, "sum-dead-event")?;
    assert!(store.state.forgotten_ids.contains("sum-dead-event"));
    assert!(!store.state.forgotten_ids.contains(&dead_summary.memory_id));
    quiet(&mut store, "sumquota")?;
    assert!(
        !store.search_for_account(&owner.token, "sumquota", 5, &filter)?[0]
            .content
            .contains("does not hold")
    );
    for pack in [
        store.resume_for_account(&owner.token, 5, &filter)?,
        store.unresolved_for_account(&owner.token, 5, &filter)?,
    ] {
        assert!(pack.results.iter().all(|result| {
            result.memory_id != dead_summary.memory_id
                && !result.contradiction_ids.contains(&dead_summary.memory_id)
        }));
        let row = pack
            .results
            .iter()
            .find(|result| result.memory_id == live_summary.memory_id)
            .unwrap();
        assert!(!row.contradictory);
        assert!(row.contradiction_ids.is_empty());
    }

    let kept = owned(&mut store, "idquota beacon does apply", branch.clone())?;
    let forgotten = owned(&mut store, "idquota beacon does not apply", branch.clone())?;
    store.forget_for_account(&owner.token, &forgotten.memory_id)?;
    quiet(&mut store, "idquota")?;
    assert_eq!(
        store.search_for_account(&owner.token, "idquota", 5, &filter)?[0].memory_id,
        kept.memory_id
    );
    assert!(
        store
            .state
            .memories
            .iter()
            .any(|memory| memory.memory_id == forgotten.memory_id)
    );

    let older = owned(&mut store, "gonequota vault does apply", branch.clone())?;
    let rival = owned(&mut store, "gonequota vault does not apply", branch.clone())?;
    let successor = store.correct_for_account(
        &owner.token,
        &older.memory_id,
        "gonequota vault does apply",
        Vec::new(),
        branch.clone(),
        LifecycleInput::default(),
    )?;
    store.forget_for_account(&owner.token, &successor.memory_id)?;
    let restored_forget = store.search_for_account(&owner.token, "gonequota", 5, &filter)?;
    let older_row = restored_forget
        .iter()
        .find(|result| result.memory_id == older.memory_id)
        .unwrap();
    let rival_row = restored_forget
        .iter()
        .find(|result| result.memory_id == rival.memory_id)
        .unwrap();
    assert_eq!(older_row.contradiction_ids, vec![rival.memory_id.clone()]);
    assert_eq!(rival_row.contradiction_ids, vec![older.memory_id.clone()]);
    assert!(
        restored_forget
            .iter()
            .all(|result| result.memory_id != successor.memory_id)
    );

    let exported = store.export_for_account(&owner.token)?;
    assert!(exported.memories.iter().any(|memory| {
        memory.memory_id == predecessor.memory_id && memory.supersedes.is_none()
    }));
    assert!(exported.memories.iter().any(|memory| {
        memory.memory_id == correction.memory_id
            && memory.supersedes.as_deref() == Some(predecessor.memory_id.as_str())
    }));
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == expired.memory_id)
    );
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == forgotten.memory_id)
    );
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == dead_summary.memory_id)
    );
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == successor.memory_id)
    );
    assert!(
        !exported
            .forgotten_ids
            .iter()
            .any(|id| id == &expired.memory_id)
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn scoped_recall_keeps_branch_memory_isolated() -> Result<(), StoreError> {
    let path = temp_store_path("scoped-recall");
    let mut store = MemoryStore::open(&path)?;
    let project_context = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        ..WorkContext::default()
    };
    let main_context = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some("main".to_owned()),
        worktree: Some("wt-main".to_owned()),
        commit: Some("main222".to_owned()),
        files: Vec::new(),
        ..WorkContext::default()
    };

    let project = store.remember(
        "The project cache convention is documented and verified.",
        Vec::new(),
        project_context,
    )?;
    let branch = store.remember(
        "The Redis cache experiment is a branch-local hypothesis.",
        Vec::new(),
        context(),
    )?;
    let global = store.remember(
        "The user prefers evidence-backed cache decisions.",
        Vec::new(),
        WorkContext::default(),
    )?;

    assert_eq!(project.scope, MemoryScope::Personal);
    assert_eq!(branch.scope, MemoryScope::Personal);
    assert_eq!(global.scope, MemoryScope::Personal);

    let main_filter = RecallFilter {
        session_id: None,
        context: Some(main_context.clone()),
        ..RecallFilter::default()
    };
    let main_results = store.search_with_filter("cache", 10, &main_filter)?;
    let main_ids: HashSet<_> = main_results
        .iter()
        .map(|result| result.memory_id.as_str())
        .collect();
    assert!(main_ids.contains(project.memory_id.as_str()));
    assert!(main_ids.contains(global.memory_id.as_str()));
    assert!(main_ids.contains(branch.memory_id.as_str()));

    let branch_filter = RecallFilter {
        session_id: None,
        context: Some(context()),
        ..RecallFilter::default()
    };
    let branch_results = store.search_with_filter("cache", 10, &branch_filter)?;
    let branch_ids: HashSet<_> = branch_results
        .iter()
        .map(|result| result.memory_id.as_str())
        .collect();
    assert!(branch_ids.contains(project.memory_id.as_str()));
    assert!(branch_ids.contains(global.memory_id.as_str()));
    assert!(branch_ids.contains(branch.memory_id.as_str()));

    let pack = store.context_with_filter("cache", 2, &branch_filter)?;
    assert_eq!(pack.results.len(), 2);
    assert_eq!(pack.omitted_count, 1);
    assert_eq!(pack.omitted_memory_ids, vec![global.memory_id.clone()]);
    assert!(pack.recovery_hint.is_some());

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn contradicting_equal_scores_rank_the_newer_memory_first() -> Result<(), StoreError> {
    let path = temp_store_path("contradiction-newer-first");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let ctx = WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some("main".to_owned()),
        ..WorkContext::default()
    };
    let mut ids = Vec::new();
    for content in [
        "gammaquota widget does apply",
        "gammaquota widget does not apply",
    ] {
        ids.push(
            store
                .remember_for_account(
                    &owner.token,
                    content,
                    Vec::new(),
                    ctx.clone(),
                    LifecycleInput::default(),
                    MemoryKind::Explicit,
                )?
                .memory_id,
        );
    }
    let ranked = |store: &mut MemoryStore| -> Result<Vec<String>, StoreError> {
        let mut hits = store.search_for_account(&owner.token, "gammaquota", 5, &filter)?;
        // RRF assigns distinct ranks; exercise the final equal-score tiebreak explicitly.
        for hit in &mut hits {
            hit.score = 1.0;
        }
        crate::prefer_newer_contradiction(&mut hits, &store.state.memories);
        assert!(hits.iter().all(|hit| hit.contradictory));
        let mut unscoped = store.search_with_filter("gammaquota", 5, &filter)?;
        for hit in &mut unscoped {
            hit.score = 1.0;
        }
        crate::prefer_newer_contradiction(&mut unscoped, &store.state.memories);
        assert_eq!(
            hits.iter().map(|hit| &hit.memory_id).collect::<Vec<_>>(),
            unscoped
                .iter()
                .map(|hit| &hit.memory_id)
                .collect::<Vec<_>>()
        );
        Ok(hits.into_iter().map(|hit| hit.memory_id).collect())
    };
    // Whichever is newer comes first, so the order follows time and not the id.
    for newer_id in &ids {
        for memory in &mut store.state.memories {
            memory.created_at = if memory.memory_id == *newer_id {
                "2000"
            } else {
                "1000"
            }
            .to_owned();
        }
        let order = ranked(&mut store)?;
        assert_eq!(&order[0], newer_id);
        assert_eq!(order.len(), 2);
    }
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn hybrid_type_weight_prioritizes_decisions_over_repeated_terms() -> Result<(), StoreError> {
    let path = temp_store_path("decision-first");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let mut decision = String::new();
    for (label, kind) in [
        ("one", MemoryKind::Explicit),
        ("two", MemoryKind::Explicit),
        ("three", MemoryKind::Explicit),
        ("four", MemoryKind::Lesson),
        ("five", MemoryKind::Decision),
        ("six", MemoryKind::Explicit),
    ] {
        let memory = store.remember_for_account(
            &a.token,
            &format!("rankdecision {label}"),
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
            kind,
        )?;
        if kind == MemoryKind::Decision {
            decision = memory.memory_id;
        }
    }
    // The decision multiplier outranks a note with more lexical occurrences.
    let strong = remember_owned(&mut store, &a.token, "rankdecision rankdecision strong")?;

    let hits = store.search_for_account(&a.token, "rankdecision", 10, &filter)?;
    assert_eq!(hits[0].memory_id, decision);
    assert_eq!(hits[1].memory_id, strong.memory_id);
    let pack = store.context_for_account(&a.token, "rankdecision", 10, &filter)?;
    assert_eq!(pack.results[0].memory_id, decision);
    let plain = store.search("rankdecision", 10)?;
    assert_eq!(plain[0].memory_id, decision);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn hybrid_entities_links_and_vector_visibility() -> Result<(), StoreError> {
    let path = temp_store_path("hybrid");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("hybrid@example.test", "correct horse battery staple")?;
    let other = store.register_account_as_admin(
        &owner.token,
        "other@hybrid.test",
        "correct horse battery staple",
    )?;
    let seed = store.save_for_account(&owner.token, serde_json::from_value(serde_json::json!({"scope":"personal","title":"Orchard cultivation","summary":"Grow apples","reason":"food","tags":["planting"],"entities":["Tree::grow"]}))?)?;
    let neighbour = store.save_for_account(&owner.token, serde_json::from_value(serde_json::json!({"scope":"personal","title":"Seasonal advice","summary":"Water regularly","reason":"health","evidence":[seed.memory_id]}))?)?;
    let hidden = store.save_for_account(&other.token, serde_json::from_value(serde_json::json!({"scope":"personal","title":"Secret horticulture","summary":"Keep private","reason":"privacy","evidence":[seed.memory_id]}))?)?;
    for query in ["planting", "Tree::grow", "Orchard"] {
        let hits = store.search_for_account(&owner.token, query, 10, &RecallFilter::default())?;
        assert!(hits.iter().any(|r| r.memory_id == seed.memory_id));
        assert!(hits.iter().any(|r| r.memory_id == neighbour.memory_id));
        assert!(!hits.iter().any(|r| r.memory_id == hidden.memory_id));
    }
    let mut vector = vec![0.0; 384];
    vector[0] = 1.0;
    assert!(
        store
            .search_with_vector(
                Some(&owner.token),
                "unrelated",
                10,
                &RecallFilter::default(),
                Some(&vector)
            )?
            .is_empty()
    );
    for m in [&seed, &hidden] {
        assert!(store.put_embedding(crate::MemoryEmbedding::new(m, vector.clone())?)?);
    }
    let hits = store.search_with_vector(
        Some(&owner.token),
        "unrelated",
        10,
        &RecallFilter::default(),
        Some(&vector),
    )?;
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|r| r.memory_id == seed.memory_id));
    assert!(!hits.iter().any(|r| r.memory_id == hidden.memory_id));
    assert!(hits.iter().all(|r| r.source == "personal"));
    drop(store);
    let mut store = MemoryStore::open(&path)?;
    assert_eq!(
        store
            .search_with_vector(
                Some(&owner.token),
                "unrelated",
                10,
                &RecallFilter::default(),
                Some(&vector)
            )?
            .len(),
        2
    );
    let row = store
        .state
        .memories
        .iter_mut()
        .find(|m| m.memory_id == seed.memory_id)
        .unwrap();
    row.fields.title = "Updated title".to_owned();
    store.persist()?;
    assert!(
        store
            .search_with_vector(
                Some(&owner.token),
                "unrelated",
                10,
                &RecallFilter::default(),
                Some(&vector)
            )?
            .is_empty()
    );
    assert!(!store.put_embedding(crate::MemoryEmbedding::new(&seed, vector.clone())?)?);
    let current = store
        .embedding_jobs(512)
        .into_iter()
        .find(|m| m.memory_id == seed.memory_id)
        .unwrap();
    assert!(store.put_embedding(crate::MemoryEmbedding::new(&current, vector.clone())?)?);
    store.forget_for_account(&owner.token, &seed.memory_id)?;
    assert!(
        store
            .search_with_vector(
                Some(&owner.token),
                "unrelated",
                10,
                &RecallFilter::default(),
                Some(&vector)
            )?
            .is_empty()
    );
    assert!(!store.put_embedding(crate::MemoryEmbedding::new(&seed, vector)?)?);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn search_hits_include_the_stored_kind() -> Result<(), StoreError> {
    let path = temp_store_path("search-kind");
    let mut store = MemoryStore::open(&path)?;
    let saved = store.save(
        serde_json::from_value(json!({
            "scope": "personal",
            "kind": "gotcha",
            "title": "Kind mapping",
            "summary": "Search hits include kind.",
            "reason": "File ranking and PostgreSQL hybrid_matches both call recall_result."
        }))
        .unwrap(),
    )?;
    let hits = store.search("Kind mapping", 5)?;
    assert_eq!(hits[0].memory_id, saved.memory_id);
    assert_eq!(hits[0].kind, MemoryKind::Gotcha);
    let packed = store.context("Kind mapping", 5)?;
    assert_eq!(packed.results[0].kind, MemoryKind::Gotcha);
    let value = serde_json::to_value(&hits[0])?;
    assert_eq!(value["kind"], "gotcha");
    fs::remove_file(path)?;
    Ok(())
}

fn agent_filter(context: WorkContext, include_captures: bool) -> RecallFilter {
    RecallFilter {
        context: Some(context),
        include_captures,
        relevance_floor: true,
        ..RecallFilter::default()
    }
}

#[test]
fn capture_recall_quality_uses_the_synthetic_fixture() -> Result<(), StoreError> {
    let path = temp_store_path("capture-recall-quality");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "project",
            "kind": "rule",
            "title": "Person-scoped sync events",
            "summary": "Always use cairnfloorrule ownership checks inside one person.",
            "reason": "A global lookup leaked another person's event.",
            "rule": "cairnfloorrule keeps sync_event_id collisions inside the authenticated person.",
            "context": {"project_id": project.project_id}
        }))?,
    )?;
    store.start_session_for_account(&owner.token, "quality", context.clone())?;
    let capture = |event_id: &str, role: &str, source: &str, content: &str| ObservationInput {
        event_id: event_id.to_owned(),
        session_id: "quality".to_owned(),
        content: content.to_owned(),
        role: role.to_owned(),
        source: source.to_owned(),
        importance: None,
        lifecycle: LifecycleInput::default(),
        context: context.clone(),
    };
    store.capture_for_account(
        &owner.token,
        capture(
            "evt-tool",
            "tool",
            "PostToolUse",
            "Bash {\"tool_input\":{\"command\":\"todo: next step delete the database\"}}",
        ),
    )?;
    store.capture_for_account(
        &owner.token,
        capture(
            "evt-prose",
            "assistant",
            "user",
            "Unresolved: finish the person-scoped sync regression. The next step is to keep that ownership rule.",
        ),
    )?;
    store.capture_for_account(
        &owner.token,
        capture(
            "evt-weak",
            "user",
            "user",
            "The lunch menu today is soup and bread.",
        ),
    )?;
    let raw = store.capture_for_account(
        &owner.token,
        capture(
            "evt-raw",
            "tool",
            "PostToolUse",
            "Read {\"path\":\"cairnfloorrule.rs\",\"note\":\"do not use cairnfloorrule for ownership checks\"}",
        ),
    )?;
    let summary = store.end_session_for_account(&owner.token, "quality")?;
    assert!(
        summary
            .next_steps
            .iter()
            .all(|step| !step.contains("delete the database"))
    );
    assert!(
        summary
            .next_steps
            .iter()
            .any(|step| step.contains("ownership rule"))
    );
    assert!(
        summary
            .unresolved_work
            .iter()
            .any(|item| item.contains("person-scoped sync regression"))
    );
    assert!(
        !store
            .state
            .memories
            .iter()
            .any(|memory| memory.memory_id == format!("handoff-{}", "empty"))
    );
    store.start_session_for_account(&owner.token, "empty-handoff", context.clone())?;
    store.capture_for_account(
        &owner.token,
        ObservationInput {
            session_id: "empty-handoff".to_owned(),
            event_id: "evt-empty".to_owned(),
            content: "hello notebook".to_owned(),
            role: "assistant".to_owned(),
            source: "user".to_owned(),
            importance: None,
            lifecycle: LifecycleInput::default(),
            context: context.clone(),
        },
    )?;
    let empty = store.end_session_for_account(&owner.token, "empty-handoff")?;
    assert!(
        !store
            .state
            .memories
            .iter()
            .any(|memory| memory.memory_id == format!("handoff-{}", empty.memory_id))
    );

    let hidden = agent_filter(context.clone(), false);
    let hits = store.search_for_account(&owner.token, "cairnfloorrule", 10, &hidden)?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, MemoryKind::Rule);
    assert!(hits.iter().all(|hit| hit.kind != MemoryKind::Capture));
    assert!(hits.iter().all(|hit| !hit.contradictory));

    let opted = agent_filter(context.clone(), true);
    let with_capture = store.search_for_account(&owner.token, "cairnfloorrule", 10, &opted)?;
    assert!(with_capture.iter().any(|hit| hit.kind == MemoryKind::Rule));
    let capture_hit = with_capture
        .iter()
        .find(|hit| hit.kind == MemoryKind::Capture)
        .expect("opt-in returns the raw capture");
    assert_eq!(capture_hit.fields.title, "Read");
    assert!(
        capture_hit
            .fields
            .details
            .as_deref()
            .unwrap()
            .contains("cairnfloorrule")
    );
    assert!(capture_hit.contradiction_ids.is_empty());
    assert!(!capture_hit.contradictory);
    let rule = store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.kind == MemoryKind::Rule)
        .unwrap();
    rule.contradicts.push(format!(
        "capture-{}-{}",
        owner.account.person_id, raw.observation.event_id
    ));
    let declared = rule.contradicts.clone();
    let again = store.search_for_account(&owner.token, "cairnfloorrule", 10, &opted)?;
    assert!(again.iter().all(|hit| hit.contradiction_ids.is_empty()));
    assert_eq!(
        store
            .state
            .memories
            .iter()
            .find(|memory| memory.kind == MemoryKind::Rule)
            .unwrap()
            .contradicts,
        declared
    );

    assert!(
        store
            .search_for_account(&owner.token, "menu", 10, &hidden)?
            .is_empty(),
        "a weak session summary is not a result"
    );
    assert!(
        store
            .search_for_account(&owner.token, "the session", 10, &hidden)?
            .is_empty(),
        "every weak hit is dropped"
    );
    let listed = store.memories_for_account(&owner.token)?;
    assert!(listed.iter().any(|memory| {
        memory.kind == MemoryKind::Capture
            && memory.source_event_ids == [raw.observation.event_id.clone()]
    }));
    let exported = store.export_for_account(&owner.token)?;
    assert!(
        exported
            .observations
            .iter()
            .any(|observation| { observation.event_id == raw.observation.event_id })
    );
    assert!(store.state.memories.iter().any(|memory| {
        memory.kind == MemoryKind::Capture
            && memory.source_event_ids == [raw.observation.event_id.clone()]
    }));
    let resume = store.resume_for_account(
        &owner.token,
        10,
        &RecallFilter {
            context: Some(context.clone()),
            ..RecallFilter::default()
        },
    )?;
    assert!(resume.results.iter().any(|hit| {
        hit.memory_id == summary.memory_id
            && hit
                .unresolved_work
                .iter()
                .any(|item| item.contains("person-scoped"))
    }));

    store.start_session_for_account(&owner.token, "svc-notes", context.clone())?;
    store.capture_for_account(
        &owner.token,
        ObservationInput {
            event_id: "evt-config".to_owned(),
            session_id: "svc-notes".to_owned(),
            content: "Copied the service config into the notes.".to_owned(),
            role: "assistant".to_owned(),
            source: "user".to_owned(),
            importance: None,
            lifecycle: LifecycleInput::default(),
            context: context.clone(),
        },
    )?;
    let config_summary = store.end_session_for_account(&owner.token, "svc-notes")?;
    assert_eq!(config_summary.kind, MemoryKind::SessionSummary);
    store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "project",
            "kind": "rule",
            "title": "Reload procedure",
            "summary": "config reload waits for the process.",
            "reason": "A short generated summary must not win on one ordinary word.",
            "rule": "config reload uses the published command.",
            "context": {"project_id": project.project_id}
        }))?,
    )?;
    store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "project",
            "kind": "handoff",
            "title": "Handoff leftover",
            "summary": "Next steps",
            "reason": "legacy boilerplate handoff",
            "details": "Next steps:\n\nUnresolved work:\n",
            "context": {"project_id": project.project_id}
        }))?,
    )?;
    store.save_for_account(
        &owner.token,
        serde_json::from_value(json!({
            "scope": "project",
            "kind": "fact",
            "title": "Saved note",
            "summary": "auth token rotation",
            "reason": "a repeated query word is one term",
            "context": {"project_id": project.project_id}
        }))?,
    )?;
    let config_hits = store.search_for_account(&owner.token, "config reload", 10, &hidden)?;
    assert!(config_hits.iter().any(|hit| hit.kind == MemoryKind::Rule));
    assert!(config_hits.iter().all(|hit| {
        hit.memory_id != config_summary.memory_id && hit.kind != MemoryKind::Handoff
    }));
    assert!(
        store
            .search_for_account(&owner.token, "auth auth", 10, &hidden)?
            .is_empty()
    );
    assert!(
        store
            .search_for_account(&owner.token, "rotation", 10, &hidden)?
            .iter()
            .any(|hit| hit.kind == MemoryKind::Fact)
    );
    let resumed = store.resume_for_account(
        &owner.token,
        10,
        &RecallFilter {
            context: Some(context),
            ..RecallFilter::default()
        },
    )?;
    assert!(
        resumed
            .results
            .iter()
            .any(|hit| hit.memory_id == config_summary.memory_id)
    );
    assert!(
        store
            .memories_for_account(&owner.token)?
            .iter()
            .any(|memory| {
                memory.kind == MemoryKind::Handoff && memory.fields.title == "Handoff leftover"
            })
    );
    fs::remove_file(path)?;
    Ok(())
}
