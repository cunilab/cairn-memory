use crate::capture::ObservationInput;
use crate::lifecycle::{LifecycleInput, MemoryScope, Sensitivity};
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{
    age_observations, context, context_from_fixture, ended, observation, observation_for,
    repo_context, summaries, temp_store_path,
};
use crate::types::{StoreError, WorkContext};
use serde_json::Value;
use std::fs;

#[test]
fn captures_and_recalls_evidence_backed_session_memory() -> Result<(), StoreError> {
    let path = temp_store_path("continuity");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-auth-001", context())?;
    store.capture_observation(observation(
        "evt-session-002",
        "The session middleware validates the signed cookie before loading the user.",
    ))?;
    store.capture_observation(observation(
        "evt-session-003",
        "The missing signature test passed.",
    ))?;

    let memory = store.end_session("sess-auth-001")?;
    assert!(memory.content.contains("missing signature"));
    assert_eq!(memory.source_event_ids.len(), 2);
    assert_eq!(memory.scope, MemoryScope::Personal);
    assert_eq!(memory.context.symbols, context().symbols);
    assert_eq!(memory.context.commands, context().commands);
    assert_eq!(memory.context.tests, context().tests);
    assert_eq!(memory.context.services, context().services);

    let results = store.search("missing signature", 5)?;
    assert_eq!(results.len(), 2, "session note and separate handoff");
    assert!(
        results
            .iter()
            .any(|m| m.memory_id == "memory-sess-auth-001")
    );
    assert!(
        results[0]
            .source_event_ids
            .contains(&"evt-session-003".to_owned())
    );
    assert_eq!(results[0].scope, MemoryScope::Personal);
    assert!(
        !store
            .search("SessionMiddleware validate_cookie", 5)?
            .is_empty()
    );

    let _ = fs::remove_file(path);
    Ok(())
}

#[test]
fn session_handoff_extracts_next_steps_and_unresolved_work() -> Result<(), StoreError> {
    let path = temp_store_path("handoff");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-handoff", context())?;
    store.capture_observation(observation_for(
        "sess-handoff",
        "evt-handoff",
        "The next step is to add the missing signature regression test; the issue is not yet verified.",
    ))?;

    let memory = store.end_session("sess-handoff")?;
    assert_eq!(memory.next_steps.len(), 1);
    assert_eq!(memory.unresolved_work.len(), 1);
    assert!(memory.content.contains("Next steps:"));
    assert!(memory.content.contains("Unresolved work:"));

    let filter = RecallFilter {
        session_id: None,
        context: Some(context()),
    };
    let resume = store.resume_context(5, &filter)?;
    assert_eq!(resume.results.len(), 1);
    assert_eq!(resume.results[0].next_steps, memory.next_steps);

    let unresolved = store.unresolved_context(5, &filter)?;
    assert_eq!(unresolved.results.len(), 1);
    assert_eq!(
        unresolved.results[0].unresolved_work,
        memory.unresolved_work
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn failed_end_session_leaves_the_session_open() -> Result<(), StoreError> {
    let path = temp_store_path("end-session-rollback");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-empty", context())?;

    store
        .end_session("sess-empty")
        .expect_err("a session with no observations cannot end");
    assert!(store.sessions().all(|session| session.ended_at.is_none()));

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn reopening_store_preserves_session_memory() -> Result<(), StoreError> {
    let path = temp_store_path("restart");
    {
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-auth-001", context())?;
        store.capture_observation(observation(
            "evt-restart",
            "restart persistence keeps the session evidence",
        ))?;
        store.end_session("sess-auth-001")?;
    }

    let store = MemoryStore::open(&path)?;
    let results = store.search("restart persistence", 5)?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].source_event_ids, vec!["evt-restart"]);

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn session_continuity_fixture_runs_through_capture_to_recall() -> Result<(), StoreError> {
    let path = temp_store_path("session-fixture");
    let mut store = MemoryStore::open(&path)?;
    let mut memory = None;

    for line in include_str!("../../../../fixtures/phase-0/session-continuity.jsonl").lines() {
        let record: Value = serde_json::from_str(line).expect("fixture record must be JSON");
        let event_type = record["type"].as_str().expect("fixture type");
        let session_id = record["sessionId"].as_str().expect("fixture session");
        let context = context_from_fixture(&record["context"]);

        match event_type {
            "session.start" => {
                store.start_session(session_id, context)?;
            }
            "observation" | "test" => {
                let payload = &record["payload"];
                let content = payload["content"]
                    .as_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "{} {} {:?}",
                            payload["command"].as_str().unwrap_or_default(),
                            payload["result"].as_str().unwrap_or_default(),
                            payload["tests"]
                        )
                    });
                store.capture_observation(ObservationInput {
                    event_id: record["eventId"].as_str().unwrap().to_owned(),
                    session_id: session_id.to_owned(),
                    content,
                    role: payload["role"].as_str().unwrap_or("tool").to_owned(),
                    source: payload["source"].as_str().unwrap_or(event_type).to_owned(),
                    importance: None,
                    lifecycle: LifecycleInput::default(),
                    context,
                })?;
            }
            "session.end" => {
                memory = Some(store.end_session(session_id)?);
            }
            other => panic!("unexpected fixture event type: {other}"),
        }
    }

    let memory = memory.expect("fixture must end the session");
    assert_eq!(memory.source_event_ids.len(), 2);
    let pack = store.context("missing signature", 5)?;
    assert_eq!(pack.results.len(), 2, "session note and separate handoff");
    assert!(
        pack.results[0]
            .source_event_ids
            .contains(&"evt-session-003".to_owned())
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn session_summary_deduplicates_repeated_observations() -> Result<(), StoreError> {
    let path = temp_store_path("compression");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-compress", context())?;
    for event_id in ["evt-compress-a", "evt-compress-b"] {
        store.capture_observation(ObservationInput {
            event_id: event_id.to_owned(),
            session_id: "sess-compress".to_owned(),
            content: "The same verified result was reported twice.".to_owned(),
            role: "tool".to_owned(),
            source: "test".to_owned(),
            importance: Some("medium".to_owned()),
            lifecycle: LifecycleInput::default(),
            context: context(),
        })?;
    }
    let summary = store.end_session("sess-compress")?;
    assert!(summary.content.contains("deterministically compressed"));
    assert!(summary.content.contains("1 repeated"));
    assert!(summary.content.contains("Source actors"));
    assert!(summary.content.contains("test"));
    assert_eq!(summary.source_event_ids.len(), 2);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn capture_finishes_only_this_persons_idle_sessions() -> Result<(), StoreError> {
    let path = temp_store_path("auto-close-idle");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let b = store.register_account("b@example.test", "correct horse battery staple")?;
    let (a_id, b_id) = (a.account.person_id.clone(), b.account.person_id.clone());
    for (token, session) in [
        (&a.token, "idle-a"),
        (&a.token, "fresh-a"),
        (&b.token, "idle-b"),
    ] {
        store.start_session_for_account(token, session, WorkContext::default())?;
        store.capture_for_account(
            token,
            observation_for(session, &format!("evt-{session}"), "idlefact"),
        )?;
    }
    store.start_session_for_account(&a.token, "empty-a", WorkContext::default())?;
    age_observations(&mut store, "idle-a", 31 * 60);
    age_observations(&mut store, "idle-b", 31 * 60);
    age_observations(&mut store, "fresh-a", 29 * 60);
    assert!(!ended(&store, &a_id, "idle-a"));

    store.capture_for_account(
        &a.token,
        observation_for("trigger-a", "evt-trigger", "trigger"),
    )?;

    assert!(ended(&store, &a_id, "idle-a"));
    assert_eq!(summaries(&store, "idle-a").len(), 1);
    assert!(store.memory_owned_by(&summaries(&store, "idle-a")[0].memory_id.clone(), &a_id));
    assert!(!ended(&store, &a_id, "fresh-a"), "29 minutes is not idle");
    assert!(
        !ended(&store, &a_id, "empty-a"),
        "no observations: skipped, no error"
    );
    assert!(summaries(&store, "empty-a").is_empty());
    assert!(
        !ended(&store, &b_id, "idle-b"),
        "another person is never swept"
    );
    assert!(summaries(&store, "idle-b").is_empty());
    // A session start by the other person sweeps only that person.
    store.start_session_for_account(&b.token, "next-b", WorkContext::default())?;
    assert!(ended(&store, &b_id, "idle-b"));
    assert!(!ended(&store, &a_id, "fresh-a"));
    // The sweep is persisted.
    let reopened = MemoryStore::open(&path)?;
    assert!(ended(&reopened, &a_id, "idle-a") && ended(&reopened, &b_id, "idle-b"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn reused_session_id_gets_a_fresh_summary() -> Result<(), StoreError> {
    let path = temp_store_path("auto-close-reuse");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let a_id = a.account.person_id.clone();
    store.start_session_for_account(&a.token, "reuse", WorkContext::default())?;
    store.capture_for_account(
        &a.token,
        observation_for("reuse", "evt-1", "firstreusefact"),
    )?;
    let first = store.end_session_for_account(&a.token, "reuse")?;
    assert_eq!(first.source_event_ids.len(), 1);
    // The agent keeps using the ended session id.
    store.capture_for_account(
        &a.token,
        observation_for("reuse", "evt-2", "secondreusefact"),
    )?;
    // Idle sweep: the stale summary is derived again and replaced.
    age_observations(&mut store, "reuse", 31 * 60);
    store.capture_for_account(&a.token, observation_for("other", "evt-3", "other"))?;
    let summary = summaries(&store, "reuse");
    assert_eq!(summary.len(), 1, "replaced, not duplicated");
    assert_eq!(summary[0].memory_id, first.memory_id);
    assert_eq!(summary[0].source_event_ids.len(), 2);
    assert!(summary[0].content.contains("secondreusefact"));
    // An unchanged summary is left alone by a repeat.
    let same = store.end_session_for_account(&a.token, "reuse")?;
    assert_eq!(&same, summaries(&store, "reuse")[0]);
    // An explicit end re-derives too.
    store.capture_for_account(
        &a.token,
        observation_for("reuse", "evt-4", "thirdreusefact"),
    )?;
    let third = store.end_session_for_account(&a.token, "reuse")?;
    assert_eq!(third.source_event_ids.len(), 3);
    assert!(third.content.contains("thirdreusefact"));
    assert_eq!(summaries(&store, "reuse").len(), 1);
    assert!(store.memory_owned_by(&third.memory_id, &a_id));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn starting_a_session_on_a_shared_branch_leaves_other_open_sessions_alone() -> Result<(), StoreError>
{
    let path = temp_store_path("no-close-on-start");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let a_id = a.account.person_id.clone();
    let main = repo_context("cairn-demo", Some("main"));
    store.start_session_for_account(&a.token, "s1", main.clone())?;
    store.capture_for_account(&a.token, observation_for("s1", "evt-s1", "branchfact"))?;
    // A parallel agent on the same repository and branch keeps its fresh session.
    store.start_session_for_account(&a.token, "s2", main)?;
    assert!(!ended(&store, &a_id, "s1"));
    assert!(summaries(&store, "s1").is_empty());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_summary_the_person_hid_is_never_derived_again() -> Result<(), StoreError> {
    let path = temp_store_path("hidden-summary");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    for (session, how) in [("forgot", "forget"), ("fixed", "correct")] {
        store.start_session_for_account(&a.token, session, WorkContext::default())?;
        store.capture_for_account(
            &a.token,
            observation_for(session, &format!("{session}-1"), "hiddenfact"),
        )?;
        let summary = store.end_session_for_account(&a.token, session)?;
        if how == "forget" {
            store.forget_for_account(&a.token, &summary.memory_id)?;
        } else {
            store.correct_for_account(
                &a.token,
                &summary.memory_id,
                "corrected text",
                Vec::new(),
                context(),
                LifecycleInput::default(),
            )?;
        }
        store.capture_for_account(
            &a.token,
            observation_for(session, &format!("{session}-2"), "morefact"),
        )?;
        age_observations(&mut store, session, 31 * 60);
        store.capture_for_account(
            &a.token,
            observation_for("trigger", &format!("t-{session}"), "trigger"),
        )?;
        let kept = summaries(&store, session);
        assert_eq!(kept.len(), 1, "{how}: not replaced");
        assert_eq!(
            kept[0].source_event_ids.len(),
            1,
            "{how}: not derived again"
        );
        // An explicit end reports the hidden summary instead of returning it as success.
        let error = store
            .end_session_for_account(&a.token, session)
            .expect_err("hidden summary");
        assert!(
            error.to_string().contains("forgotten or superseded"),
            "{how}: {error}"
        );
    }
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_rederived_summary_keeps_verification_lifecycle_and_creation_time() -> Result<(), StoreError> {
    let path = temp_store_path("rederive-keeps-state");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    store.start_session_for_account(&a.token, "keep", WorkContext::default())?;
    store.capture_for_account(&a.token, observation_for("keep", "keep-1", "keepfact"))?;
    store.end_session_for_account(&a.token, "keep")?;
    let id = summaries(&store, "keep")[0].memory_id.clone();
    for memory in &mut store.state.memories {
        if memory.memory_id == id {
            memory.verified_commit = Some("verified1".to_owned());
            memory.created_at = "1000".to_owned();
            memory.lifecycle.freshness = 42;
            memory.lifecycle.sensitivity = Sensitivity::Secret;
        }
    }
    store.capture_for_account(&a.token, observation_for("keep", "keep-2", "againfact"))?;
    let again = store.end_session_for_account(&a.token, "keep")?;
    assert_eq!(again.source_event_ids.len(), 2);
    assert_eq!(again.verified_commit.as_deref(), Some("verified1"));
    assert_eq!(again.created_at, "1000");
    assert_eq!(again.lifecycle.freshness, 42);
    assert_eq!(again.lifecycle.sensitivity, Sensitivity::Secret);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_summary_with_a_forgotten_source_is_derived_again_without_it() -> Result<(), StoreError> {
    let path = temp_store_path("forgotten-source");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    store.start_session_for_account(&a.token, "src", WorkContext::default())?;
    store.capture_for_account(&a.token, observation_for("src", "src-1", "forgetmeword"))?;
    store.capture_for_account(&a.token, observation_for("src", "src-2", "keepmeword"))?;
    let first = store.end_session_for_account(&a.token, "src")?;
    assert!(first.content.contains("forgetmeword"));
    store.forget_for_account(&a.token, "src-1")?;
    let filter = RecallFilter::default();
    assert!(
        store
            .search_for_account(&a.token, "keepmeword", 5, &filter)?
            .is_empty(),
        "hidden with its source"
    );
    age_observations(&mut store, "src", 31 * 60);
    store.capture_for_account(&a.token, observation_for("trigger", "t-src", "trigger"))?;
    let hits = store.search_for_account(&a.token, "keepmeword", 5, &filter)?;
    assert_eq!(hits.len(), 1);
    assert!(!hits[0].content.contains("forgetmeword"));
    assert_eq!(hits[0].source_event_ids, vec!["src-2".to_owned()]);
    assert_eq!(summaries(&store, "src").len(), 1);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn periodic_sweep_closes_idle_sessions_for_every_person() -> Result<(), StoreError> {
    let path = temp_store_path("sweep-all");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let b = store.register_account("b@example.test", "correct horse battery staple")?;
    let (a_id, b_id) = (a.account.person_id.clone(), b.account.person_id.clone());
    for (token, session) in [
        (&a.token, "sweep-a"),
        (&a.token, "fresh-a"),
        (&b.token, "sweep-b"),
    ] {
        store.start_session_for_account(token, session, WorkContext::default())?;
        store.capture_for_account(
            token,
            observation_for(session, &format!("evt-{session}"), "sweepfact"),
        )?;
    }
    age_observations(&mut store, "sweep-a", 31 * 60);
    age_observations(&mut store, "sweep-b", 31 * 60);
    age_observations(&mut store, "fresh-a", 29 * 60);

    store.sweep_idle_sessions();

    assert!(ended(&store, &a_id, "sweep-a") && ended(&store, &b_id, "sweep-b"));
    assert!(!ended(&store, &a_id, "fresh-a"), "29 minutes is not idle");
    assert_eq!(summaries(&store, "sweep-a").len(), 1);
    assert_eq!(summaries(&store, "sweep-b").len(), 1);
    store.sweep_idle_sessions();
    assert_eq!(summaries(&store, "sweep-a").len(), 1, "no second summary");
    let reopened = MemoryStore::open(&path)?;
    assert!(ended(&reopened, &b_id, "sweep-b"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn hook_only_push_creates_the_session_and_the_sweep_summarises_it() -> Result<(), StoreError> {
    let path = temp_store_path("push-only-session");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let a_id = a.account.person_id.clone();
    let device = store.register_device("dev-hook", &a_id, None, "Hook")?;
    store.sync_push(
        &device.token,
        observation_for("hook-only", "evt-hook-only", "hookonlyfact"),
    )?;
    assert!(!ended(&store, &a_id, "hook-only"));
    age_observations(&mut store, "hook-only", 31 * 60);

    store.sweep_idle_sessions();

    assert!(ended(&store, &a_id, "hook-only"));
    assert_eq!(summaries(&store, "hook-only").len(), 1);
    // The same push replayed does not add a second session row.
    store.sync_push(
        &device.token,
        observation_for("hook-only", "evt-hook-only", "hookonlyfact"),
    )?;
    let rows = store
        .state
        .sessions
        .iter()
        .filter(|session| session.session_id == "hook-only")
        .count();
    assert_eq!(rows, 1);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_push_gives_the_session_its_context_so_the_summary_is_repository_scoped()
-> Result<(), StoreError> {
    let path = temp_store_path("push-context-scope");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-pc", &a.account.person_id, None, "Hook")?;
    let hook_context = |branch: &str| WorkContext {
        repository: Some("cairn-demo".to_owned()),
        branch: Some(branch.to_owned()),
        ..WorkContext::default()
    };
    let mut push = |session: &str, event: &str, context: WorkContext| {
        let mut input = observation_for(session, event, "pushscopefact");
        input.context = context;
        store.sync_push(&device.token, input)
    };
    push("created", "evt-created", hook_context("feat/hook"))?;
    // A push into an empty row fills it; a later push never replaces a real context.
    push("filled", "evt-filled-1", WorkContext::default())?;
    push("filled", "evt-filled-2", hook_context("feat/hook"))?;
    push("filled", "evt-filled-3", hook_context("feat/other"))?;
    for session in ["created", "filled"] {
        age_observations(&mut store, session, 31 * 60);
    }

    store.sweep_idle_sessions();

    for session in ["created", "filled"] {
        let summary = summaries(&store, session).remove(0);
        assert_eq!(summary.scope, MemoryScope::Personal, "{session}");
        assert_eq!(summary.context.branch.as_deref(), Some("feat/hook"));
    }
    let filter = RecallFilter {
        context: Some(hook_context("feat/hook")),
        ..RecallFilter::default()
    };
    let hits = store.search_with_filter("pushscopefact", 10, &filter)?;
    assert!(
        hits.iter()
            .any(|hit| hit.memory_id == summaries(&store, "created")[0].memory_id),
        "a repository/branch-filtered recall finds the hook-only summary"
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_start_after_a_push_fills_the_empty_session_context() -> Result<(), StoreError> {
    let path = temp_store_path("push-then-start");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-ps", &a.account.person_id, None, "Hook")?;
    let mut input = observation_for("push-start", "evt-push-start", "pushstartfact");
    input.context = WorkContext::default();
    store.sync_push(&device.token, input)?;
    let started = store.start_session_for_account(&a.token, "push-start", context())?;
    assert_eq!(started.context.branch.as_deref(), Some("feat/auth"));
    // A real context is kept: the next start does not replace it.
    let again = store.start_session_for_account(&a.token, "push-start", WorkContext::default())?;
    assert_eq!(again.context.branch.as_deref(), Some("feat/auth"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn summaries_never_broaden_personal_or_mixed_project_sources() {
    let source = |project: Option<&str>, event: &str| {
        let mut input = crate::test_support::observation_for("mixed", event, "captured evidence");
        input.context.project_id = project.map(str::to_owned);
        crate::normalize_observation(&input).unwrap()
    };
    let first = source(Some("project-a"), "first");
    let summary = crate::derive_session_memory(
        Some("author"),
        "mixed",
        &first.context,
        std::slice::from_ref(&first),
    )
    .unwrap();
    assert_eq!(summary.scope, crate::MemoryScope::Project);
    for other in [source(None, "personal"), source(Some("project-b"), "other")] {
        let summary = crate::derive_session_memory(
            Some("author"),
            "mixed",
            &first.context,
            &[first.clone(), other],
        )
        .unwrap();
        assert_eq!(summary.scope, crate::MemoryScope::Personal);
        assert!(summary.project_id.is_none());
        assert!(summary.published_by.is_none());
    }
}
