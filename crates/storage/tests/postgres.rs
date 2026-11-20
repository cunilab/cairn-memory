use cairn_core::{CheckpointStatus, ObservationInput, RecallFilter, WorkContext};
use cairn_storage::{PostgresError, PostgresStore};
use sqlx::PgPool;

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_layer3_account_device_and_sync_flow() {
    let database_url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&database_url)
        .await
        .expect("connect and migrate PostgreSQL");

    let account = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .expect("bootstrap account");
    let first = store
        .register_device(
            &account.token,
            "device-a",
            &account.account.person_id,
            Some("agent-a"),
            "MacBook",
        )
        .await
        .expect("register first device");
    let resumed = store
        .register_device(
            &account.token,
            "device-a",
            &account.account.person_id,
            Some("agent-a"),
            "MacBook",
        )
        .await
        .expect("resume first device");
    assert_ne!(first.token, resumed.token);
    assert!(store.sync_pull(&first.token, 0).await.is_err());

    let second = store
        .register_device(
            &account.token,
            "device-b",
            &account.account.person_id,
            None,
            "Laptop",
        )
        .await
        .expect("register second device");
    assert_eq!(store.list_devices(&account.token).await.unwrap().len(), 2);

    let input = ObservationInput {
        event_id: "event-1".to_owned(),
        session_id: "session-1".to_owned(),
        content: "A private api_key=do-not-store was captured".to_owned(),
        role: "agent".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: Default::default(),
        context: WorkContext::default(),
    };
    let pushed = store
        .sync_push(&resumed.token, input.clone())
        .await
        .expect("push observation");
    assert!(!pushed.duplicate);
    assert!(!pushed.observation.content.contains("do-not-store"));

    let duplicate = store
        .sync_push(&resumed.token, input.clone())
        .await
        .expect("replay observation");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.cursor, pushed.cursor);

    // The same id with different content is a conflict, not an acknowledged duplicate.
    let conflicting = store
        .sync_push(
            &resumed.token,
            ObservationInput {
                content: "a different observation under the same event id".to_owned(),
                ..input.clone()
            },
        )
        .await
        .expect_err("a changed replay conflicts");
    assert!(
        matches!(conflicting, PostgresError::Conflict(_)),
        "{conflicting}"
    );
    // A pure replay writes no audit row.
    let raw = PgPool::connect(&database_url).await.expect("audit pool");
    let replays: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_audit_events WHERE action = 'sync_duplicate'",
    )
    .fetch_one(&raw)
    .await
    .expect("count replay audit rows");
    assert_eq!(replays, 0);

    let pulled = store
        .sync_pull(&second.token, 0)
        .await
        .expect("pull observation");
    assert_eq!(pulled.events.len(), 1);
    assert_eq!(pulled.events[0].event_id, "event-1");

    let summary = store
        .end_session(&account.token, "session-1")
        .await
        .expect("derive session memory");
    assert_eq!(summary.kind, cairn_core::MemoryKind::SessionSummary);
    let explicit = store
        .remember_with_kind(
            &account.token,
            "PostgreSQL retrieval must preserve person ownership",
            vec!["server authority".to_owned()],
            WorkContext {
                repository: Some("cairn".to_owned()),
                ..WorkContext::default()
            },
            Default::default(),
            cairn_core::MemoryKind::Explicit,
        )
        .await
        .expect("store explicit memory");
    let project = store
        .register_project(
            &account.token,
            "cairn",
            "Cairn Memory",
            Some("main".to_owned()),
        )
        .await
        .expect("register project");
    let branch = store
        .register_branch(
            &account.token,
            &project.project_id,
            "feature/cache",
            Some("main".to_owned()),
            Some("base123".to_owned()),
            Some("abc1234".to_owned()),
        )
        .await
        .expect("register branch");
    assert_eq!(
        store
            .branches(&account.token, &project.project_id)
            .await
            .unwrap()
            .len(),
        1
    );
    let worktree = store
        .register_worktree(
            &account.token,
            &project.project_id,
            Some(branch.branch_id.clone()),
            "/worktrees/cache",
            Some("abc1234".to_owned()),
        )
        .await
        .expect("register worktree");
    assert_eq!(
        worktree.branch_id.as_deref(),
        Some(branch.branch_id.as_str())
    );
    assert_eq!(
        store
            .worktrees(&account.token, &project.project_id)
            .await
            .unwrap()
            .len(),
        1
    );
    let closable_branch = store
        .register_branch(
            &account.token,
            &project.project_id,
            "feature/obsolete",
            Some("main".to_owned()),
            None,
            None,
        )
        .await
        .expect("register closable branch");
    let closed_branch = store
        .close_branch(
            &account.token,
            &project.project_id,
            &closable_branch.branch_id,
            "Superseded before merge",
        )
        .await
        .expect("close branch");
    assert_eq!(closed_branch.status, "closed");
    let branch_memory = store
        .remember_with_kind(
            &account.token,
            "The cache decision passed the branch regression test",
            vec!["cache".to_owned()],
            WorkContext {
                repository: Some("cairn".to_owned()),
                branch: Some("feature/cache".to_owned()),
                worktree: Some("wt-cache".to_owned()),
                commit: Some("abc1234".to_owned()),
                files: vec!["src/cache.rs".to_owned()],
                symbols: vec!["Cache::get".to_owned()],
                commands: vec!["cargo test cache".to_owned()],
                tests: vec!["cache_reads_are_consistent".to_owned()],
                ..WorkContext::default()
            },
            Default::default(),
            cairn_core::MemoryKind::Explicit,
        )
        .await
        .expect("store branch memory");
    let promoted = save_project_note(
        &store,
        &account.token,
        &project.project_id,
        &branch_memory.content,
        branch_memory.context.clone(),
    )
    .await;
    assert_eq!(promoted.kind, cairn_core::MemoryKind::Decision);
    assert_eq!(promoted.scope, cairn_core::MemoryScope::Project);
    assert!(promoted.promoted_from.is_none());
    let verified = store
        .verify_project_memory(
            &account.token,
            &project.project_id,
            &promoted.memory_id,
            "def5678",
        )
        .await
        .expect("verify project memory");
    assert_eq!(verified.verified_commit.as_deref(), Some("def5678"));
    assert_eq!(verified.lifecycle.freshness, 100);
    let merged = store
        .confirm_branch_merge(
            &account.token,
            &project.project_id,
            &branch.branch_id,
            &promoted.memory_id,
            "merge999",
            "Confirmed after merge into main",
        )
        .await
        .expect("confirm branch merge");
    assert_eq!(merged.branch.status, "merged");
    assert_eq!(merged.branch.merge_commit.as_deref(), Some("merge999"));
    assert_eq!(merged.memory.memory_id, promoted.memory_id);
    assert_eq!(merged.memory.verified_commit.as_deref(), Some("merge999"));
    assert_eq!(
        store
            .worktrees(&account.token, &project.project_id)
            .await
            .expect("list resolved worktrees")[0]
            .status,
        "removed"
    );
    let repeated_merge = store
        .confirm_branch_merge(
            &account.token,
            &project.project_id,
            &branch.branch_id,
            &promoted.memory_id,
            "merge999",
            "Confirmed after merge into main",
        )
        .await
        .expect("repeat branch merge");
    assert_eq!(repeated_merge.memory.memory_id, promoted.memory_id);
    assert_eq!(
        store
            .project_memories(&account.token, &project.project_id)
            .await
            .expect("list project memories")
            .len(),
        1
    );
    let entities = store
        .project_entities(&account.token, &project.project_id)
        .await
        .expect("list project entities");
    assert!(
        entities
            .iter()
            .any(|entity| entity.kind == "file" && entity.key == "src/cache.rs")
    );
    assert!(
        entities
            .iter()
            .any(|entity| entity.kind == "symbol" && entity.key == "Cache::get")
    );
    assert!(
        entities
            .iter()
            .any(|entity| entity.kind == "decision" && entity.key == promoted.memory_id)
    );
    let history = store
        .freshness_history(&account.token, &project.project_id)
        .await
        .expect("list freshness history");
    let actions = history
        .iter()
        .map(|event| event.action.as_str())
        .collect::<Vec<_>>();
    assert!(actions.contains(&"verification"));
    assert!(actions.contains(&"merge"));
    let stale = store
        .project_maintenance(&account.token, &project.project_id, Some("other-commit"))
        .await
        .expect("maintenance against another commit");
    assert!(stale.stale_memory_ids.contains(&promoted.memory_id));
    assert!(stale.active_memory_ids.contains(&promoted.memory_id));
    assert!(stale.superseded_memory_ids.is_empty());
    let current = store
        .project_maintenance(&account.token, &project.project_id, Some("merge999"))
        .await
        .expect("maintenance at the merge commit");
    assert!(!current.stale_memory_ids.contains(&promoted.memory_id));
    let confirmed = store
        .confirm_memory(
            &account.token,
            &project.project_id,
            &promoted.memory_id,
            Some("confirm123"),
            "Confirmed from the registry check",
        )
        .await
        .expect("confirm promoted memory");
    assert_eq!(confirmed.memory_id, promoted.memory_id);
    assert_eq!(confirmed.verified_commit.as_deref(), Some("confirm123"));
    let listed_projects = store.projects(&account.token).await.expect("list projects");
    assert_eq!(listed_projects.len(), 1);
    let results = store
        .search(
            &account.token,
            "person ownership",
            10,
            &RecallFilter::default(),
        )
        .await
        .expect("search memories");
    assert!(
        results
            .iter()
            .any(|result| result.memory_id == explicit.memory_id)
    );
    let context = store
        .context(
            &account.token,
            "person ownership",
            1,
            &RecallFilter::default(),
        )
        .await
        .expect("build context pack");
    assert_eq!(context.results.len(), 1);
    assert_eq!(context.omitted_count, 0);
    let corrected = store
        .correct(
            &account.token,
            &explicit.memory_id,
            "PostgreSQL retrieval must preserve person ownership because the server is authoritative",
            vec!["server authority".to_owned()],
            WorkContext {
                repository: Some("cairn".to_owned()),
                ..WorkContext::default()
            },
            cairn_core::LifecycleInput {
                confidence: Some(40),
                ..Default::default()
            },
        )
        .await
        .expect("correct explicit memory");
    assert_eq!(corrected.kind, cairn_core::MemoryKind::Correction);
    assert_eq!(corrected.lifecycle.confidence, 100);
    assert_eq!(
        corrected.supersedes.as_deref(),
        Some(explicit.memory_id.as_str())
    );
    let corrected_search = store
        .search(
            &account.token,
            "person ownership",
            10,
            &RecallFilter::default(),
        )
        .await
        .expect("search after correction");
    assert!(
        corrected_search
            .iter()
            .any(|result| result.memory_id == corrected.memory_id)
    );
    assert!(
        corrected_search
            .iter()
            .all(|result| result.memory_id != explicit.memory_id)
    );
    let why_changed = store
        .why_changed_context(
            &account.token,
            "server authoritative",
            10,
            &RecallFilter::default(),
        )
        .await
        .expect("build why-changed context");
    assert!(
        why_changed
            .results
            .iter()
            .any(|result| result.decision_reason.is_some())
    );
    let exported = store
        .export_bundle(&account.token)
        .await
        .expect("export PostgreSQL memory bundle");
    assert_eq!(exported.format, cairn_core::EXPORT_FORMAT);
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == explicit.memory_id)
    );
    assert!(
        !exported
            .forgotten_ids
            .iter()
            .any(|id| id == &explicit.memory_id)
    );
    let imported = store
        .import_bundle(&account.token, exported)
        .await
        .expect("idempotent PostgreSQL import");
    assert_eq!(imported.imported_memories, 0);
    let diagnostics = store
        .sync_diagnostics(&resumed.token)
        .await
        .expect("sync diagnostics");
    assert_eq!(diagnostics.server_cursor, pushed.cursor);
    assert_eq!(diagnostics.lag, pushed.cursor);
    let lifecycle = store
        .lifecycle_summary(&account.token)
        .await
        .expect("lifecycle diagnostics");
    assert!(lifecycle.durable_memories >= 2);
    let project_correction = store
        .correct(
            &account.token,
            &promoted.memory_id,
            "The cache decision passed after review.",
            Vec::new(),
            WorkContext::default(),
            cairn_core::LifecycleInput::default(),
        )
        .await
        .expect("correct promoted project memory");
    let report = store
        .project_maintenance(&account.token, &project.project_id, None)
        .await
        .expect("maintenance after project correction");
    assert!(report.superseded_memory_ids.is_empty());
    assert!(
        report
            .active_memory_ids
            .contains(&project_correction.memory_id)
    );
    assert!(!report.active_memory_ids.contains(&promoted.memory_id));
    let audit = store
        .audit_events(&account.token, 20)
        .await
        .expect("audit history");
    assert!(
        audit
            .iter()
            .any(|event| event.action == "account_bootstrap")
    );
    store
        .forget(&account.token, &explicit.memory_id)
        .await
        .expect("forget explicit memory");
    let forgotten_results = store
        .search(
            &account.token,
            "person ownership",
            10,
            &RecallFilter::default(),
        )
        .await
        .expect("search after forget");
    assert!(
        !forgotten_results
            .iter()
            .any(|result| result.memory_id == explicit.memory_id)
    );

    let rotated = store
        .rotate_device(&account.token, "device-a")
        .await
        .expect("rotate device token");
    assert!(store.sync_pull(&resumed.token, 0).await.is_err());
    assert!(store.sync_pull(&rotated.token, 0).await.is_ok());

    store
        .revoke_device(&account.token, "device-a")
        .await
        .expect("revoke device");
    assert!(store.sync_pull(&rotated.token, 0).await.is_err());

    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .await
        .expect("register member account");
    let shared_session = "shared-session";
    store
        .start_session(&account.token, shared_session, WorkContext::default())
        .await
        .expect("owner shared session");
    store
        .start_session(&member.token, shared_session, WorkContext::default())
        .await
        .expect("member shared session");
    let member_device = store
        .register_device(
            &member.token,
            "device-member-shared",
            &member.account.person_id,
            None,
            "Member",
        )
        .await
        .expect("member device");
    store
        .sync_push(
            &second.token,
            ObservationInput {
                event_id: "owner-shared-evt".to_owned(),
                session_id: shared_session.to_owned(),
                content: "alphaquux ownerfact".to_owned(),
                role: "agent".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: WorkContext::default(),
            },
        )
        .await
        .expect("owner shared push");
    store
        .sync_push(
            &member_device.token,
            ObservationInput {
                event_id: "member-shared-evt".to_owned(),
                session_id: shared_session.to_owned(),
                content: "betazet memberfact".to_owned(),
                role: "agent".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: WorkContext::default(),
            },
        )
        .await
        .expect("member shared push");
    let seen = store
        .sync_pull(&second.token, 0)
        .await
        .expect("pull before forget")
        .cursor;
    let owner_summary = store
        .end_session(&account.token, shared_session)
        .await
        .expect("owner session memory");
    let member_summary = store
        .end_session(&member.token, shared_session)
        .await
        .expect("member session memory");
    assert_ne!(owner_summary.memory_id, member_summary.memory_id);
    assert_eq!(
        owner_summary.memory_id,
        format!("memory-{}-{shared_session}", account.account.person_id)
    );
    assert_eq!(
        member_summary.memory_id,
        format!("memory-{}-{shared_session}", member.account.person_id)
    );
    assert!(owner_summary.content.contains("ownerfact"));
    assert!(!owner_summary.content.contains("memberfact"));
    let owner_hits = store
        .search(&account.token, "ownerfact", 5, &RecallFilter::default())
        .await
        .expect("owner search");
    assert!(
        owner_hits
            .iter()
            .any(|hit| hit.memory_id == owner_summary.memory_id)
    );
    let member_hits = store
        .search(&member.token, "ownerfact", 5, &RecallFilter::default())
        .await
        .expect("member search");
    assert!(
        member_hits
            .iter()
            .all(|hit| hit.memory_id != owner_summary.memory_id)
    );

    store
        .forget(&account.token, "owner-shared-evt")
        .await
        .expect("forget synced observation");
    let from_start = store
        .sync_pull(&member_device.token, 0)
        .await
        .expect("member pull after owner forget");
    assert!(
        from_start
            .events
            .iter()
            .all(|event| event.event_id != "owner-shared-evt")
    );
    let after = store
        .sync_pull(&second.token, seen)
        .await
        .expect("pull tombstone");
    assert!(
        after
            .events
            .iter()
            .all(|event| event.event_id != "owner-shared-evt")
    );
    assert!(
        after
            .deletions
            .iter()
            .any(|deletion| deletion.item_id == "owner-shared-evt")
    );
}

fn placed(branch: &str) -> WorkContext {
    WorkContext {
        repository: Some("cairn".to_owned()),
        branch: Some(branch.to_owned()),
        worktree: Some(format!("wt-{branch}")),
        ..WorkContext::default()
    }
}

fn pushed(event_id: &str, session_id: &str, context: WorkContext) -> ObservationInput {
    ObservationInput {
        event_id: event_id.to_owned(),
        session_id: session_id.to_owned(),
        content: "context ordering check".to_owned(),
        role: "agent".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: Default::default(),
        context,
    }
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_context_order_repairs_empty_session_without_rewriting_observations() {
    let database_url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&database_url)
        .await
        .expect("connect and migrate PostgreSQL");
    let account = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .expect("bootstrap account");
    let device = store
        .register_device(
            &account.token,
            "device-context",
            &account.account.person_id,
            None,
            "Context laptop",
        )
        .await
        .expect("register device");
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .await
        .expect("register other account");
    let other_device = store
        .register_device(
            &other.token,
            "device-other-context",
            &other.account.person_id,
            None,
            "Other laptop",
        )
        .await
        .expect("register other device");

    let mut started = placed("main");
    started.files = vec!["notes password=metadata-secret-6f3a".to_owned()];
    let session = store
        .start_session(&account.token, "sess-inherit", started)
        .await
        .expect("start session before an empty push");
    let stored_files = session.context.files.join("\n");
    assert!(
        !stored_files.contains("metadata-secret-6f3a"),
        "session context kept a metadata secret"
    );
    assert!(
        stored_files.contains("[REDACTED]"),
        "session context list was not redacted"
    );
    assert_eq!(session.context.branch.as_deref(), Some("main"));
    let inherited = store
        .sync_push(
            &device.token,
            pushed("evt-inherit", "sess-inherit", WorkContext::default()),
        )
        .await
        .expect("empty push after start");
    assert!(!inherited.duplicate);
    assert_eq!(
        inherited.observation.context.repository.as_deref(),
        Some("cairn")
    );
    assert_eq!(
        inherited.observation.context.branch.as_deref(),
        Some("main")
    );
    assert_eq!(
        inherited.observation.context.worktree.as_deref(),
        Some("wt-main")
    );
    assert!(
        !inherited
            .observation
            .context
            .files
            .iter()
            .any(|file| file.contains("metadata-secret-6f3a")),
        "inherited observation kept a metadata secret"
    );
    let mut replaced = placed("replaced");
    replaced.files = vec!["src/other.rs".to_owned()];
    let inherited_again = store
        .sync_push(
            &device.token,
            pushed("evt-inherit", "sess-inherit", replaced),
        )
        .await
        .expect("duplicate push");
    assert!(inherited_again.duplicate);
    assert_eq!(inherited_again.cursor, inherited.cursor);
    assert_eq!(
        inherited_again.observation.context,
        inherited.observation.context
    );
    let summary = store
        .end_session(&account.token, "sess-inherit")
        .await
        .expect("summarize inherited session");
    assert_eq!(
        summary.memory_id,
        format!("memory-{}-sess-inherit", account.account.person_id)
    );
    assert_eq!(summary.scope, cairn_core::MemoryScope::Personal);
    assert_eq!(summary.context.branch.as_deref(), Some("main"));
    assert_eq!(summary.context.worktree.as_deref(), Some("wt-main"));

    let late = store
        .sync_push(
            &device.token,
            pushed("evt-late", "sess-late", WorkContext::default()),
        )
        .await
        .expect("empty push before start");
    assert_eq!(late.observation.context, WorkContext::default());
    let filled = store
        .start_session(&account.token, "sess-late", placed("repair"))
        .await
        .expect("start fills an empty session");
    assert_eq!(filled.context, placed("repair"));
    let kept = store
        .start_session(&account.token, "sess-late", placed("later"))
        .await
        .expect("second start keeps the first context");
    assert_eq!(kept.context, placed("repair"));
    assert!(kept.ended_at.is_none());
    let late_again = store
        .sync_push(
            &device.token,
            pushed("evt-late", "sess-late", placed("should-not-stick")),
        )
        .await
        .expect("duplicate of the original empty push");
    assert!(late_again.duplicate);
    assert_eq!(late_again.cursor, late.cursor);
    assert_eq!(late_again.observation.context, WorkContext::default());
    let late_summary = store
        .end_session(&account.token, "sess-late")
        .await
        .expect("summarize the repaired session");
    assert_eq!(late_summary.scope, cairn_core::MemoryScope::Personal);
    assert_eq!(late_summary.context, placed("repair"));
    let reopened = store
        .start_session(&account.token, "sess-late", placed("after-end"))
        .await
        .expect("start after end");
    assert_eq!(reopened.context, placed("repair"));
    assert!(reopened.ended_at.is_some());

    let _ = store
        .sync_push(
            &device.token,
            pushed("evt-race", "sess-race", WorkContext::default()),
        )
        .await
        .expect("empty placeholder before concurrent starts");
    let left = placed("race-left");
    let right = placed("race-right");
    let (first, second) = tokio::join!(
        store.start_session(&account.token, "sess-race", left.clone()),
        store.start_session(&account.token, "sess-race", right.clone()),
    );
    let first = first.expect("first concurrent start");
    let second = second.expect("second concurrent start");
    assert_eq!(first.context, second.context);
    assert!(first.context == left || first.context == right);
    let third = store
        .start_session(&account.token, "sess-race", placed("race-third"))
        .await
        .expect("later start");
    assert_eq!(third.context, first.context);

    let owner_session = store
        .start_session(&account.token, "sess-foreign", placed("owner-only"))
        .await
        .expect("owner session");
    let foreign = store
        .sync_push(
            &other_device.token,
            pushed("evt-foreign", "sess-foreign", WorkContext::default()),
        )
        .await
        .expect("other person's empty push");
    assert_eq!(foreign.observation.context, WorkContext::default());
    let foreign_session = store
        .start_session(&other.token, "sess-foreign", placed("foreign-branch"))
        .await
        .expect("other person's start");
    assert_eq!(foreign_session.context, placed("foreign-branch"));
    let owner_again = store
        .start_session(&account.token, "sess-foreign", placed("owner-replaced"))
        .await
        .expect("owner start stays");
    assert_eq!(owner_again.context, owner_session.context);

    let _ = store
        .start_session(&account.token, "sess-partial", placed("session-branch"))
        .await
        .expect("partial session");
    let partial = WorkContext {
        files: vec!["src/keep.rs".to_owned()],
        ..WorkContext::default()
    };
    let partial_push = store
        .sync_push(
            &device.token,
            pushed("evt-partial", "sess-partial", partial.clone()),
        )
        .await
        .expect("partial push");
    assert_eq!(partial_push.observation.context, partial);
    let partial_session = store
        .start_session(&account.token, "sess-partial", placed("other-branch"))
        .await
        .expect("partial session is unchanged");
    assert_eq!(partial_session.context, placed("session-branch"));

    let imported = store
        .import_bundle(
            &account.token,
            cairn_core::ExportBundle {
                format: cairn_core::EXPORT_FORMAT.to_owned(),
                version: cairn_core::EXPORT_VERSION,
                exported_at: "0".to_owned(),
                sessions: vec![cairn_core::Session {
                    session_id: "sess-import".to_owned(),
                    person_id: None,
                    context: WorkContext::default(),
                    started_at: "0".to_owned(),
                    ended_at: None,
                }],
                observations: vec![cairn_core::Observation {
                    event_id: "evt-import".to_owned(),
                    session_id: "sess-import".to_owned(),
                    content: "imported outside sync".to_owned(),
                    role: "agent".to_owned(),
                    source: "import".to_owned(),
                    importance: None,
                    lifecycle: cairn_core::Lifecycle::default(),
                    scope: cairn_core::MemoryScope::Personal,
                    context: WorkContext::default(),
                    captured_at: "0".to_owned(),
                }],
                memories: Vec::new(),
                forgotten_ids: Vec::new(),
            },
        )
        .await
        .expect("import an observation with no sync event");
    assert_eq!(imported.imported_observations, 1);
    let outside = store
        .sync_push(
            &device.token,
            pushed("evt-import", "sess-import", WorkContext::default()),
        )
        .await
        .expect_err("changed text is not adopted");
    // The pushed text differs from the imported row, so it is a changed replay.
    assert!(matches!(outside, PostgresError::Conflict(_)), "{outside:?}");
    let outside = outside.to_string();
    assert!(outside.contains("event id already used with different content"));
    assert!(!outside.contains("imported outside sync"));

    let raw = PgPool::connect(&database_url).await.expect("fixture pool");
    let owner_id = account.account.person_id.as_str();
    let other_id = other.account.person_id.as_str();
    let omitted = placed("omitted-repair");
    let shape = store
        .start_session(&account.token, "sess-omitted-shape", omitted.clone())
        .await
        .expect("canonical shape");
    let shape_text = session_context_text(&raw, owner_id, "sess-omitted-shape").await;
    assert_eq!(shape.context, omitted);

    plant_session(&raw, owner_id, "sess-brace", None).await;
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-brace").await,
        "{}"
    );
    let brace_push = store
        .sync_push(
            &device.token,
            pushed("evt-brace", "sess-brace", WorkContext::default()),
        )
        .await
        .expect("push against a column-default session");
    assert_eq!(brace_push.observation.context, WorkContext::default());
    let brace_observation = observation_context_text(&raw, owner_id, "evt-brace").await;
    let brace_session = store
        .start_session(&account.token, "sess-brace", omitted.clone())
        .await
        .expect("start fills column default");
    assert_eq!(brace_session.context, omitted);
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-brace").await,
        shape_text
    );
    let brace_again = store
        .sync_push(
            &device.token,
            pushed("evt-brace", "sess-brace", omitted.clone()),
        )
        .await
        .expect("duplicate after filling the column default");
    assert!(brace_again.duplicate);
    assert_eq!(brace_again.cursor, brace_push.cursor);
    assert_eq!(brace_again.observation.context, WorkContext::default());
    assert_eq!(
        observation_context_text(&raw, owner_id, "evt-brace").await,
        brace_observation
    );

    plant_session(&raw, owner_id, "sess-files", Some(r#"{"files":[]}"#)).await;
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-files").await,
        r#"{"files": []}"#
    );
    let files_session = store
        .start_session(&account.token, "sess-files", omitted.clone())
        .await
        .expect("start fills omitted files");
    assert_eq!(files_session.context, omitted);
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-files").await,
        shape_text
    );

    plant_session(
        &raw,
        owner_id,
        "sess-kept",
        Some(r#"{"branch":"kept-branch"}"#),
    )
    .await;
    let kept_before = session_context_text(&raw, owner_id, "sess-kept").await;
    assert_eq!(kept_before, r#"{"branch": "kept-branch"}"#);
    let kept = store
        .start_session(&account.token, "sess-kept", placed("should-not-replace"))
        .await
        .expect("start leaves a real branch");
    assert_eq!(kept.context.branch.as_deref(), Some("kept-branch"));
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-kept").await,
        kept_before
    );

    plant_session(
        &raw,
        owner_id,
        "sess-extra",
        Some(r#"{"files":[],"extra":"keep-me"}"#),
    )
    .await;
    let extra_before = session_context_text(&raw, owner_id, "sess-extra").await;
    assert_eq!(extra_before, r#"{"extra": "keep-me", "files": []}"#);
    let extra = store
        .start_session(&account.token, "sess-extra", placed("should-not-erase"))
        .await
        .expect("start leaves an unknown key");
    assert_ne!(extra.context, placed("should-not-erase"));
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-extra").await,
        extra_before
    );

    plant_session(&raw, owner_id, "sess-brace-foreign", None).await;
    let foreign_fill = store
        .start_session(&other.token, "sess-brace-foreign", placed("foreign-brace"))
        .await
        .expect("other account start");
    assert_eq!(foreign_fill.context, placed("foreign-brace"));
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-brace-foreign").await,
        "{}"
    );
    let owner_fill = store
        .start_session(&account.token, "sess-brace-foreign", placed("owner-brace"))
        .await
        .expect("owner fills its own column default");
    assert_eq!(owner_fill.context, placed("owner-brace"));
    let foreign_text = session_context_text(&raw, other_id, "sess-brace-foreign").await;
    assert_ne!(
        session_context_text(&raw, owner_id, "sess-brace-foreign").await,
        foreign_text
    );
    assert!(foreign_text.contains("foreign-brace"));

    plant_session(&raw, owner_id, "sess-brace-race", None).await;
    let brace_left = placed("race-brace-left");
    let brace_right = placed("race-brace-right");
    let (brace_first, brace_second) = tokio::join!(
        store.start_session(&account.token, "sess-brace-race", brace_left.clone()),
        store.start_session(&account.token, "sess-brace-race", brace_right.clone()),
    );
    let brace_first = brace_first.expect("first column-default start");
    let brace_second = brace_second.expect("second column-default start");
    assert_eq!(brace_first.context, brace_second.context);
    assert!(brace_first.context == brace_left || brace_first.context == brace_right);
    let brace_third = store
        .start_session(
            &account.token,
            "sess-brace-race",
            placed("race-brace-third"),
        )
        .await
        .expect("later column-default start");
    assert_eq!(brace_third.context, brace_first.context);
    let winner_shape = store
        .start_session(
            &account.token,
            "sess-brace-winner",
            brace_first.context.clone(),
        )
        .await
        .expect("winner shape");
    assert_eq!(winner_shape.context, brace_first.context);
    assert_eq!(
        session_context_text(&raw, owner_id, "sess-brace-race").await,
        session_context_text(&raw, owner_id, "sess-brace-winner").await
    );

    let exported = store
        .export_bundle(&account.token)
        .await
        .expect("export after context ordering");
    let late_row = exported
        .observations
        .iter()
        .find(|observation| observation.event_id == "evt-late")
        .expect("stored late observation");
    assert_eq!(late_row.context, WorkContext::default());
    let inherited_row = exported
        .observations
        .iter()
        .find(|observation| observation.event_id == "evt-inherit")
        .expect("stored inherited observation");
    assert_eq!(inherited_row.context.branch.as_deref(), Some("main"));
    assert!(
        exported
            .sessions
            .iter()
            .all(|session| { !context_contains_secret(&session.context) })
            && exported.observations.iter().all(|observation| {
                !observation.content.contains("metadata-secret-6f3a")
                    && !context_contains_secret(&observation.context)
            })
            && exported.memories.iter().all(|memory| {
                !memory.content.contains("metadata-secret-6f3a")
                    && !context_contains_secret(&memory.context)
                    && memory
                        .next_steps
                        .iter()
                        .chain(memory.unresolved_work.iter())
                        .all(|item| !item.contains("metadata-secret-6f3a"))
            }),
        "export kept a metadata secret"
    );
}

async fn plant_session(
    pool: &PgPool,
    person_id: &str,
    session_id: &str,
    context_json: Option<&str>,
) {
    match context_json {
        None => {
            sqlx::query("INSERT INTO cairn_sessions (session_id, person_id) VALUES ($1, $2)")
                .bind(session_id)
                .bind(person_id)
                .execute(pool)
                .await
                .expect("plant column-default session");
        }
        Some(context_json) => {
            sqlx::query(
                "INSERT INTO cairn_sessions (session_id, person_id, context)
                 VALUES ($1, $2, $3::jsonb)",
            )
            .bind(session_id)
            .bind(person_id)
            .bind(context_json)
            .execute(pool)
            .await
            .expect("plant session context");
        }
    }
}

async fn session_context_text(pool: &PgPool, person_id: &str, session_id: &str) -> String {
    sqlx::query_scalar(
        "SELECT context::text FROM cairn_sessions
         WHERE person_id = $1 AND session_id = $2",
    )
    .bind(person_id)
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("session context text")
}

async fn observation_context_text(pool: &PgPool, person_id: &str, event_id: &str) -> String {
    sqlx::query_scalar(
        "SELECT context::text FROM cairn_observations
         WHERE person_id = $1 AND event_id = $2",
    )
    .bind(person_id)
    .bind(event_id)
    .fetch_one(pool)
    .await
    .expect("observation context text")
}

fn context_contains_secret(context: &WorkContext) -> bool {
    [
        context.repository.as_deref(),
        context.branch.as_deref(),
        context.worktree.as_deref(),
        context.commit.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(context.files.iter().map(String::as_str))
    .chain(context.symbols.iter().map(String::as_str))
    .chain(context.commands.iter().map(String::as_str))
    .chain(context.tests.iter().map(String::as_str))
    .chain(context.services.iter().map(String::as_str))
    .chain(context.handoffs.iter().map(String::as_str))
    .chain(context.runbooks.iter().map(String::as_str))
    .any(|value| value.contains("metadata-secret-6f3a"))
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_context_omissions_count_every_eligible_match_past_the_cap() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let raw = PgPool::connect(&url).await.expect("fixture pool");
    let owner = store
        .bootstrap_account("cap-owner@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let member = store
        .register_account("cap-member@example.test", "correct horse battery staple")
        .await
        .expect("member");
    let f = RecallFilter::default();
    let remember = |token: String, content: String, context: WorkContext| {
        let store = store.clone();
        async move {
            store
                .remember_with_kind(
                    &token,
                    &content,
                    Vec::new(),
                    context,
                    Default::default(),
                    cairn_core::MemoryKind::Explicit,
                )
                .await
                .expect("remember")
                .memory_id
        }
    };

    // 513 eligible owner matches, plus rows that must never be counted.
    for i in 0..513 {
        remember(
            owner.token.clone(),
            format!("capq row{i:04}"),
            WorkContext::default(),
        )
        .await;
    }
    for i in 0..3 {
        remember(
            member.token.clone(),
            format!("capq foreign{i}"),
            WorkContext::default(),
        )
        .await;
    }
    let forgotten = remember(
        owner.token.clone(),
        "capq forgotten".to_owned(),
        WorkContext::default(),
    )
    .await;
    store
        .forget(&owner.token, &forgotten)
        .await
        .expect("forget");
    let expired = remember(
        owner.token.clone(),
        "capq expired".to_owned(),
        WorkContext::default(),
    )
    .await;
    sqlx::query("UPDATE cairn_memories SET lifecycle = jsonb_set(lifecycle, '{expiresAt}', '\"1\"') WHERE memory_id = $1")
        .bind(&expired)
        .execute(&raw)
        .await
        .expect("expire");
    let superseded = remember(
        owner.token.clone(),
        "capq superseded".to_owned(),
        WorkContext::default(),
    )
    .await;
    store
        .correct(
            &owner.token,
            &superseded,
            "capq replaced",
            Vec::new(),
            WorkContext::default(),
            Default::default(),
        )
        .await
        .expect("correct");
    // The correction is itself a 514th eligible match.
    let total = 514usize;

    for limit in [0usize, 1, 512, 513, usize::MAX] {
        let pack = store
            .context(&owner.token, "capq", limit, &f)
            .await
            .expect("context");
        let returned = pack.results.len();
        assert_eq!(returned, limit.min(512), "returned rows at limit {limit}");
        assert_eq!(
            pack.omitted_count,
            total - returned,
            "omitted_count at limit {limit}"
        );
        assert!(
            pack.omitted_memory_ids.len() < pack.omitted_count,
            "ids are a bounded subset at limit {limit}"
        );
        assert_eq!(
            pack.omitted_memory_ids.len(),
            512 - returned,
            "ids stay inside the 512 window at limit {limit}"
        );
        for id in &pack.omitted_memory_ids {
            assert!(
                !pack.results.iter().any(|r| &r.memory_id == id),
                "omitted id also returned"
            );
            assert!(
                ![&forgotten, &expired, &superseded].contains(&id),
                "ineligible id listed"
            );
        }
        let hint = pack.recovery_hint.expect("hint when rows are omitted");
        assert!(
            hint.contains("512") && hint.to_lowercase().contains("narrow"),
            "hint names the cap and narrowing: {hint}"
        );
        assert!(
            !hint.contains("Increase the limit"),
            "hint must not promise a larger limit recovers rows: {hint}"
        );
    }

    // A filter changes the count with the same predicate as the rows.
    for name in ["branchrow one", "branchrow two"] {
        remember(
            owner.token.clone(),
            format!("capq {name}"),
            placed("feat-x"),
        )
        .await;
    }
    for (branch, expected) in [("feat-x", total + 2), ("feat-y", total + 2)] {
        let filter = RecallFilter {
            context: Some(placed(branch)),
            ..Default::default()
        };
        let pack = store
            .context(&owner.token, "capq", 1, &filter)
            .await
            .expect("filtered");
        assert_eq!(
            pack.omitted_count,
            expected - 1,
            "filtered omitted_count for {branch}"
        );
    }

    // No match: exact zero, no hint.
    let none = store
        .context(&owner.token, "zzznomatch", 0, &f)
        .await
        .expect("none");
    assert_eq!(
        (
            none.omitted_count,
            none.omitted_memory_ids.len(),
            none.recovery_hint
        ),
        (0, 0, None)
    );

    // Why-changed ranks decision evidence across every eligible match before the 512 cut.
    // The rank-513 lexical decision must lead, and hidden decisions must not count or win.
    let decision = remember(
        owner.token.clone(),
        "whyq decided because latency".to_owned(),
        WorkContext::default(),
    )
    .await;
    for i in 0..512 {
        remember(
            owner.token.clone(),
            format!("whyq whyq whyq filler{i:04}"),
            WorkContext::default(),
        )
        .await;
    }
    let hidden_forgotten = remember(
        owner.token.clone(),
        "whyq whyq whyq whyq forgotten because hidden".to_owned(),
        WorkContext::default(),
    )
    .await;
    store
        .forget(&owner.token, &hidden_forgotten)
        .await
        .expect("forget decision");
    let hidden_expired = remember(
        owner.token.clone(),
        "whyq whyq whyq whyq expired because hidden".to_owned(),
        WorkContext::default(),
    )
    .await;
    sqlx::query(
        "UPDATE cairn_memories SET lifecycle = jsonb_set(lifecycle, '{expiresAt}', '\"1\"') WHERE memory_id = $1",
    )
    .bind(&hidden_expired)
    .execute(&raw)
    .await
    .expect("expire decision");
    let hidden_superseded = remember(
        owner.token.clone(),
        "whyq whyq whyq whyq superseded because hidden".to_owned(),
        WorkContext::default(),
    )
    .await;
    // The correction is a plain 514th eligible match and not a decision.
    store
        .correct(
            &owner.token,
            &hidden_superseded,
            "whyq replacement",
            Vec::new(),
            WorkContext::default(),
            Default::default(),
        )
        .await
        .expect("correct decision");
    remember(
        member.token.clone(),
        "whyq whyq whyq whyq foreign because rival".to_owned(),
        WorkContext::default(),
    )
    .await;
    let why_total = 514usize;
    for limit in [0usize, 1, 5, 512, 513, usize::MAX] {
        let why = store
            .why_changed_context(&owner.token, "whyq", limit, &f)
            .await
            .expect("why");
        let returned = why.results.len();
        assert_eq!(
            returned,
            limit.min(512),
            "why-changed rows at limit {limit}"
        );
        assert_eq!(
            why.omitted_count,
            why_total - returned,
            "why-changed omitted at limit {limit}"
        );
        assert_eq!(why.omitted_memory_ids.len(), 512 - returned);
        if returned > 0 {
            assert_eq!(
                why.results[0].memory_id, decision,
                "rank-513 decision leads at limit {limit}"
            );
            assert!(why.results[0].decision_reason.is_some());
            assert!(why.results[1..].iter().all(|r| r.decision_reason.is_none()));
        }
        let rendered = format!("{why:?}");
        assert!(
            !rendered.contains("hidden") && !rendered.contains("rival"),
            "ineligible decision surfaced at limit {limit}"
        );
        assert!(
            ![&hidden_forgotten, &hidden_expired, &hidden_superseded]
                .iter()
                .any(|id| rendered.contains(id.as_str()))
        );
    }
    // Plain search and context keep lexical order: the decision (rank 513 by score) stays outside.
    let lexical = store
        .search(&owner.token, "whyq", usize::MAX, &f)
        .await
        .expect("search");
    assert_eq!(lexical.len(), 512);
    assert!(
        lexical.iter().all(|r| r.memory_id != decision),
        "plain search stays lexical"
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_why_changed_orders_like_the_file_store() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let owner = store
        .bootstrap_account("why-owner@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let file_path =
        std::env::temp_dir().join(format!("cairn-why-order-{}.json", std::process::id()));
    let mut file = cairn_core::MemoryStore::open(&file_path).expect("file store");
    let files = |names: &[&str]| WorkContext {
        files: names.iter().map(|name| (*name).to_owned()).collect(),
        ..WorkContext::default()
    };
    // Insert order is the id tie order. Groups: decision by relationship count, then plain.
    let fixtures: Vec<(&str, WorkContext)> = vec![
        ("whyo plain whyo", files(&[])),
        ("whyo decided because one", files(&["a.rs"])),
        ("whyo decided because zero first", files(&[])),
        ("whyo plain two", files(&["a.rs", "b.rs"])),
        ("whyo decided because two", files(&["a.rs", "b.rs"])),
        ("whyo decided because zero second", files(&[])),
        ("whyo whyo whyo plain strong", files(&[])),
    ];
    for (content, context) in &fixtures {
        store
            .remember_with_kind(
                &owner.token,
                content,
                Vec::new(),
                context.clone(),
                Default::default(),
                cairn_core::MemoryKind::Explicit,
            )
            .await
            .expect("pg remember");
        file.remember(content, Vec::new(), context.clone())
            .expect("file remember");
    }
    let f = RecallFilter::default();
    let content_order = |results: &[cairn_core::RecallResult]| -> Vec<String> {
        results.iter().map(|r| r.content.clone()).collect()
    };
    let pg = store
        .why_changed_context(&owner.token, "whyo", 10, &f)
        .await
        .expect("pg why");
    let reference = file.why_changed_context("whyo", 10, &f).expect("file why");
    assert_eq!(
        content_order(&pg.results),
        content_order(&reference.results)
    );
    assert_eq!(pg.results.len(), fixtures.len());
    // Decision groups lead, by relationship count, ties by insertion (id) order.
    let reasons: Vec<bool> = pg
        .results
        .iter()
        .map(|r| r.decision_reason.is_some())
        .collect();
    assert_eq!(reasons, [true, true, true, true, false, false, false]);
    let relations: Vec<usize> = pg.results.iter().map(|r| r.relationships.len()).collect();
    assert!(relations[0] >= relations[1] && relations[1] >= relations[2]);
    // Limit 1 and 5: same prefix, exact omissions.
    for limit in [0usize, 1, 5] {
        let pack = store
            .why_changed_context(&owner.token, "whyo", limit, &f)
            .await
            .expect("pg why limit");
        assert_eq!(
            content_order(&pack.results),
            content_order(&reference.results)[..limit]
        );
        assert_eq!(pack.omitted_count, fixtures.len() - limit);
        assert_eq!(pack.omitted_memory_ids.len(), fixtures.len() - limit);
    }
    let _ = std::fs::remove_file(&file_path);
}

// CM-AUDIT PG registry D2: branch and worktree registration check the caller's own project
// first, so a missing or foreign project is a validation error, not a raw foreign-key failure.
#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_branch_and_worktree_registration_validate_project_membership() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let password = "correct horse battery staple";
    let owner = store
        .bootstrap_account("reg-owner@example.test", password)
        .await
        .expect("owner");
    let outsider = store
        .register_account("reg-outsider@example.test", password)
        .await
        .expect("outsider");
    let project = store
        .register_project(&owner.token, "acme/app", "App", None)
        .await
        .expect("project")
        .project_id;
    let other = store
        .register_project(&owner.token, "acme/other", "Other", None)
        .await
        .expect("second project")
        .project_id;

    // Owner: normal registration and idempotence.
    let branch = store
        .register_branch(&owner.token, &project, "feat/x", None, None, None)
        .await
        .expect("branch");
    let again = store
        .register_branch(&owner.token, &project, "feat/x", None, None, None)
        .await
        .expect("idempotent branch");
    assert_eq!(again.branch_id, branch.branch_id);
    let worktree = store
        .register_worktree(
            &owner.token,
            &project,
            Some(branch.branch_id.clone()),
            "/wt/a",
            None,
        )
        .await
        .expect("worktree");
    let worktree_again = store
        .register_worktree(
            &owner.token,
            &project,
            Some(branch.branch_id.clone()),
            "/wt/a",
            None,
        )
        .await
        .expect("idempotent worktree");
    assert_eq!(worktree_again.worktree_id, worktree.worktree_id);

    // Both a foreign project and a missing project refuse registration without leaking schema details.
    let missing = "project-missing".to_owned();
    for (token, id) in [
        (&outsider.token, &project),
        (&outsider.token, &missing),
        (&owner.token, &missing),
    ] {
        let expected = if id == &missing {
            format!("unknown project: {id}")
        } else {
            "account is not a project member".to_owned()
        };
        let branch_error = store
            .register_branch(token, id, "feat/evil", None, None, None)
            .await
            .expect_err("branch registration needs membership");
        assert!(
            matches!(&branch_error, PostgresError::Validation(text) if *text == expected),
            "branch: {branch_error}"
        );
        let worktree_error = store
            .register_worktree(token, id, None, "/wt/evil", None)
            .await
            .expect_err("worktree registration needs membership");
        assert!(
            matches!(&worktree_error, PostgresError::Validation(text) if *text == expected),
            "worktree: {worktree_error}"
        );
        // With a branch id as well, the project is checked first: same error, no schema text.
        let with_branch = store
            .register_worktree(token, id, Some(branch.branch_id.clone()), "/wt/evil", None)
            .await
            .expect_err("worktree with a branch id");
        assert!(
            !with_branch.to_string().contains("violates"),
            "{with_branch}"
        );
    }
    // Auth first: a bad token is an authentication failure, not a project error.
    let bad = store
        .register_branch("not-a-token", &project, "feat/evil", None, None, None)
        .await
        .expect_err("bad token");
    assert!(bad.is_unauthorized());

    // The same person's branch of another project is still refused, and nothing was written.
    let cross = store
        .register_worktree(
            &owner.token,
            &other,
            Some(branch.branch_id.clone()),
            "/wt/x",
            None,
        )
        .await
        .expect_err("a branch of another project");
    assert!(
        matches!(&cross, PostgresError::Validation(text) if text.starts_with("unknown project branch"))
    );
    let branches = store
        .branches(&owner.token, &project)
        .await
        .expect("branches");
    assert_eq!(branches.len(), 1);
    assert_eq!(
        store
            .worktrees(&owner.token, &project)
            .await
            .expect("worktrees")
            .len(),
        1
    );
    assert!(
        store
            .branches(&owner.token, &other)
            .await
            .expect("other")
            .is_empty()
    );
    assert!(store.branches(&outsider.token, &project).await.is_err());
}

// An `expiresAt` above i64::MAX would pass a u64 parse but fail the `::bigint` cast in every
// read query. New remember, sync and import writes must be rejected before they persist.
#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_rejects_expiry_above_i64_before_persisting() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let owner = store
        .bootstrap_account("expiry-owner@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let device = store
        .register_device(
            &owner.token,
            "expiry-device",
            &owner.account.person_id,
            None,
            "expiry",
        )
        .await
        .expect("device");
    let lifecycle = |value: &str| cairn_core::LifecycleInput {
        expires_at: Some(value.to_owned()),
        ..Default::default()
    };
    let observation = |event: &str, value: &str| ObservationInput {
        event_id: event.to_owned(),
        session_id: "expiry-session".to_owned(),
        content: "expiryfact observation".to_owned(),
        role: "agent".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: lifecycle(value),
        context: WorkContext::default(),
    };
    let remember = |content: &'static str, value: &str| {
        store.remember_with_kind(
            &owner.token,
            content,
            Vec::new(),
            WorkContext::default(),
            lifecycle(value),
            cairn_core::MemoryKind::Explicit,
        )
    };

    remember("expiryfact good", "1900000000")
        .await
        .expect("good remember");
    store
        .sync_push(&device.token, observation("expiry-good", "1900000000"))
        .await
        .expect("good sync");
    let before = store.export_bundle(&owner.token).await.expect("export");

    for bad in ["9223372036854775808", "18446744073709551615"] {
        let mut imported = before.clone();
        imported.memories[0].lifecycle.expires_at = Some(bad.to_owned());
        let mut imported_observation = before.clone();
        imported_observation.observations[0].lifecycle.expires_at = Some(bad.to_owned());
        let results = [
            remember("expiryfact bad", bad).await.map(drop),
            store
                .sync_push(&device.token, observation("expiry-bad", bad))
                .await
                .map(drop),
            store.import_bundle(&owner.token, imported).await.map(drop),
            store
                .import_bundle(&owner.token, imported_observation)
                .await
                .map(drop),
        ];
        for result in results {
            assert!(
                matches!(
                    result,
                    Err(PostgresError::Core(cairn_core::StoreError::Validation(_)))
                ),
                "{bad}: {result:?}"
            );
        }
    }

    // i64::MAX is the largest value the read queries can cast, so it stays accepted.
    let max = i64::MAX.to_string();
    remember("expiryfact max", &max)
        .await
        .expect("max remember");
    store
        .sync_push(&device.token, observation("expiry-max", &max))
        .await
        .expect("max sync");

    let after = store.export_bundle(&owner.token).await.expect("export");
    assert_eq!(after.memories.len(), before.memories.len() + 1);
    assert_eq!(after.observations.len(), before.observations.len() + 1);
    let found = store
        .search(&owner.token, "expiryfact", 10, &RecallFilter::default())
        .await
        .expect("search");
    assert_eq!(found.len(), 2);
    store
        .lifecycle_summary(&owner.token)
        .await
        .expect("diagnose");
    store
        .end_session(&owner.token, "expiry-session")
        .await
        .expect("session end");
}

async fn pool_and_store() -> (PgPool, PostgresStore) {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let pool = PgPool::connect(&url).await.expect("pool");
    (pool, store)
}

async fn device_for(
    store: &PostgresStore,
    account: &cairn_core::AuthSessionResult,
    device_id: &str,
) -> String {
    store
        .register_device(
            &account.token,
            device_id,
            &account.account.person_id,
            None,
            device_id,
        )
        .await
        .expect("device")
        .token
}

async fn push_in(
    store: &PostgresStore,
    device: &str,
    session: &str,
    event: &str,
    ctx: WorkContext,
) {
    let mut input = pushed(event, session, ctx);
    input.content = format!("autofact {event}");
    store.sync_push(device, input).await.expect("push");
}

/// Makes the account a project member the way the committed project file does: one device
/// push that carries the project id.
async fn join_project(
    store: &PostgresStore,
    account: &cairn_core::AuthSessionResult,
    project_id: &str,
) {
    store
        .join_project(&account.token, project_id)
        .await
        .unwrap();
}

async fn age(pool: &PgPool, person_id: &str, session_id: &str) {
    sqlx::query(
        "UPDATE cairn_observations SET captured_at = now() - interval '31 minutes'
         WHERE person_id = $1 AND session_id = $2",
    )
    .bind(person_id)
    .bind(session_id)
    .execute(pool)
    .await
    .expect("age observations");
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes'
         WHERE person_id = $1 AND session_id = $2 AND pending_since IS NOT NULL",
    )
    .bind(person_id)
    .bind(session_id)
    .execute(pool)
    .await
    .expect("age pending");
}

async fn session_ended(pool: &PgPool, person_id: &str, session_id: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT ended_at IS NOT NULL FROM cairn_sessions WHERE person_id = $1 AND session_id = $2",
    )
    .bind(person_id)
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("session row")
}

async fn summary_sources(pool: &PgPool, person_id: &str, session_id: &str) -> Vec<usize> {
    sqlx::query_scalar::<_, i32>(
        "SELECT jsonb_array_length(source_event_ids) FROM cairn_memories
         WHERE person_id = $1 AND session_id = $2 AND kind = 'session_summary'",
    )
    .bind(person_id)
    .bind(session_id)
    .fetch_all(pool)
    .await
    .expect("summaries")
    .into_iter()
    .map(|count| count as usize)
    .collect()
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_pull_pages_never_skip_events_or_deletions() {
    let (pool, store) = pool_and_store().await;
    let account = store
        .bootstrap_account("pull-page@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let person = account.account.person_id.clone();
    let device = device_for(&store, &account, "pull-device").await;
    // 600 deletions first, then 3 events: the events carry the highest cursors.
    sqlx::query(
        "INSERT INTO cairn_sync_deletions (person_id, item_id, cursor)
         SELECT $1, 'gone-' || g,
                nextval(pg_get_serial_sequence('cairn_sync_events', 'cursor'))
         FROM generate_series(1, 600) g",
    )
    .bind(&person)
    .execute(&pool)
    .await
    .expect("deletions");
    for n in 0..3 {
        push_in(
            &store,
            &device,
            "pull-session",
            &format!("pull-evt-{n}"),
            WorkContext::default(),
        )
        .await;
    }
    let (mut cursor, mut events, mut deletions) = (0, 0, 0);
    for _ in 0..8 {
        let page = store.sync_pull(&device, cursor).await.expect("pull");
        assert!(page.cursor >= cursor);
        events += page.events.len();
        deletions += page.deletions.len();
        let done = page.events.is_empty() && page.deletions.is_empty();
        cursor = page.cursor;
        if done {
            break;
        }
    }
    assert_eq!(
        (events, deletions),
        (3, 600),
        "no item skipped across pages"
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_auto_closes_idle_sessions_per_person_and_rederives_reused_ids() {
    let (pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let a = store
        .bootstrap_account("auto-a@example.test", pw)
        .await
        .expect("a");
    let b = store
        .register_account("auto-b@example.test", pw)
        .await
        .expect("b");
    let (a_id, b_id) = (a.account.person_id.clone(), b.account.person_id.clone());
    let (dev_a, dev_b) = (
        device_for(&store, &a, "auto-a").await,
        device_for(&store, &b, "auto-b").await,
    );

    store
        .start_session(&a.token, "empty-a", WorkContext::default())
        .await
        .expect("start");
    push_in(&store, &dev_a, "idle-a", "ia-1", WorkContext::default()).await;
    push_in(&store, &dev_a, "fresh-a", "fa-1", WorkContext::default()).await;
    push_in(&store, &dev_b, "idle-b", "ib-1", WorkContext::default()).await;
    age(&pool, &a_id, "idle-a").await;
    age(&pool, &b_id, "idle-b").await;
    assert!(!session_ended(&pool, &a_id, "idle-a").await);

    push_in(&store, &dev_a, "trigger-a", "ta-1", WorkContext::default()).await;
    assert!(session_ended(&pool, &a_id, "idle-a").await);
    assert_eq!(summary_sources(&pool, &a_id, "idle-a").await, vec![1]);
    assert!(!session_ended(&pool, &a_id, "fresh-a").await, "not idle");
    assert!(
        !session_ended(&pool, &a_id, "empty-a").await,
        "no observations: skipped"
    );
    assert!(summary_sources(&pool, &a_id, "empty-a").await.is_empty());
    assert!(
        !session_ended(&pool, &b_id, "idle-b").await,
        "another person is not swept"
    );
    assert!(summary_sources(&pool, &b_id, "idle-b").await.is_empty());
    store
        .start_session(&b.token, "next-b", WorkContext::default())
        .await
        .expect("start b");
    assert!(session_ended(&pool, &b_id, "idle-b").await);

    // A reused session id: the summary is derived again and replaced.
    store
        .start_session(&a.token, "reuse", WorkContext::default())
        .await
        .expect("start");
    push_in(&store, &dev_a, "reuse", "ru-1", WorkContext::default()).await;
    store.end_session(&a.token, "reuse").await.expect("end");
    assert_eq!(summary_sources(&pool, &a_id, "reuse").await, vec![1]);
    push_in(&store, &dev_a, "reuse", "ru-2", WorkContext::default()).await;
    age(&pool, &a_id, "reuse").await;
    push_in(&store, &dev_a, "trigger-a", "ta-2", WorkContext::default()).await;
    assert_eq!(
        summary_sources(&pool, &a_id, "reuse").await,
        vec![2],
        "replaced, not duplicated"
    );
    push_in(&store, &dev_a, "reuse", "ru-3", WorkContext::default()).await;
    store
        .end_session(&a.token, "reuse")
        .await
        .expect("end again");
    assert_eq!(summary_sources(&pool, &a_id, "reuse").await, vec![3]);

    // A start on the same repository and branch leaves a parallel agent's session alone.
    store
        .start_session(&a.token, "s1", placed("main"))
        .await
        .expect("s1");
    push_in(&store, &dev_a, "s1", "s1-1", placed("main")).await;
    store
        .start_session(&a.token, "s2", placed("main"))
        .await
        .expect("s2");
    assert!(!session_ended(&pool, &a_id, "s1").await);
    assert!(summary_sources(&pool, &a_id, "s1").await.is_empty());
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_capture_stays_personal_until_an_explicit_project_attaches() {
    let (pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let owner = store
        .bootstrap_account("reg-o@example.test", pw)
        .await
        .expect("owner");
    let member = store
        .register_account("reg-m@example.test", pw)
        .await
        .expect("member");
    let solo = store
        .register_account("reg-s@example.test", pw)
        .await
        .expect("solo");
    let ctx = |repo: &str, branch: Option<&str>| WorkContext {
        repository: Some(repo.to_owned()),
        branch: branch.map(ToOwned::to_owned),
        ..WorkContext::default()
    };
    let solo_dev = device_for(&store, &solo, "reg-solo").await;
    push_in(
        &store,
        &solo_dev,
        "rs",
        "r1",
        ctx("  auto-pg ", Some("feat/x")),
    )
    .await;
    push_in(
        &store,
        &solo_dev,
        "rs",
        "r2",
        ctx("auto-pg", Some("feat/x")),
    )
    .await;
    push_in(&store, &solo_dev, "rs", "r3", ctx("auto-pg", Some("HEAD"))).await;
    push_in(&store, &solo_dev, "rs", "r4", ctx("auto-pg", None)).await;
    let projects = store.projects(&solo.token).await.expect("projects");
    assert!(
        projects.is_empty(),
        "a repository alone never creates a project"
    );

    let shared = store
        .register_project(&owner.token, "shared-pg", "Shared", None)
        .await
        .expect("shared");
    join_project(&store, &member, &shared.project_id).await;
    let member_dev = device_for(&store, &member, "reg-member").await;
    push_in(
        &store,
        &member_dev,
        "rm",
        "m1",
        WorkContext {
            project_id: Some(shared.project_id.clone()),
            ..ctx("shared-pg", Some("feat/m"))
        },
    )
    .await;
    let mine = store
        .projects(&member.token)
        .await
        .expect("member projects");
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].project_id, shared.project_id);
    let count = |sql: &'static str, person: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(person)
                .fetch_one(&pool)
                .await
                .expect("count")
        }
    };
    assert_eq!(
        count(
            "SELECT count(*) FROM cairn_projects WHERE person_id = $1",
            member.account.person_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM cairn_project_branches WHERE person_id = $1",
            member.account.person_id.clone()
        )
        .await,
        1
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_contradicting_equal_ranks_put_the_newer_memory_first() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("contra@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let ctx = WorkContext {
        repository: Some("contra-pg".to_owned()),
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
                .remember_with_kind(
                    &owner.token,
                    content,
                    Vec::new(),
                    ctx.clone(),
                    Default::default(),
                    cairn_core::MemoryKind::Explicit,
                )
                .await
                .expect("remember")
                .memory_id,
        );
    }
    for newer in 0..2 {
        for (index, id) in ids.iter().enumerate() {
            let at = if index == newer {
                2_000_000_000_i64
            } else {
                1_000_000_000
            };
            sqlx::query("UPDATE cairn_memories SET created_at = to_timestamp($1::double precision) WHERE memory_id = $2")
                .bind(at)
                .bind(id)
                .execute(&pool)
                .await
                .expect("set created_at");
        }
        let hits = store
            .search(&owner.token, "gammaquota", 5, &RecallFilter::default())
            .await
            .expect("search");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.contradictory));
        let mut hits = hits;
        for hit in &mut hits {
            hit.score = 1.0;
        }
        let memories = store.memories(&owner.token).await.unwrap();
        cairn_core::prefer_newer_contradiction(&mut hits, &memories);
        assert_eq!(hits[0].memory_id, ids[newer]);
    }
}

async fn summary_state(pool: &PgPool, person_id: &str, session_id: &str) -> (String, i64, i64) {
    sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT COALESCE(verified_commit, ''), EXTRACT(EPOCH FROM created_at)::bigint,
                (lifecycle->>'confidence')::bigint
         FROM cairn_memories
         WHERE person_id = $1 AND session_id = $2 AND kind = 'session_summary'",
    )
    .bind(person_id)
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("summary state")
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_rederived_summary_keeps_state_and_respects_hidden_summaries() {
    let (pool, store) = pool_and_store().await;
    let account = store
        .bootstrap_account("rederive@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let person = account.account.person_id.clone();
    let dev = device_for(&store, &account, "rederive-dev").await;
    let ctx = WorkContext::default;

    // Re-deriving keeps verification, lifecycle and creation time.
    push_in(&store, &dev, "keep", "keep-1", ctx()).await;
    store
        .end_session(&account.token, "keep")
        .await
        .expect("end");
    sqlx::query(
        "UPDATE cairn_memories SET verified_commit = 'abc123',
                created_at = now() - interval '5 days',
                lifecycle = jsonb_set(lifecycle, '{confidence}', '77')
         WHERE person_id = $1 AND session_id = 'keep' AND kind = 'session_summary'",
    )
    .bind(&person)
    .execute(&pool)
    .await
    .expect("plant state");
    let planted = summary_state(&pool, &person, "keep").await;
    push_in(&store, &dev, "keep", "keep-2", ctx()).await;
    store
        .end_session(&account.token, "keep")
        .await
        .expect("end again");
    assert_eq!(summary_sources(&pool, &person, "keep").await, vec![2]);
    assert_eq!(summary_state(&pool, &person, "keep").await, planted);

    // A forgotten source: the summary is derived again without it by the automatic close.
    push_in(&store, &dev, "src", "src-1", ctx()).await;
    push_in(&store, &dev, "src", "src-2", ctx()).await;
    store.end_session(&account.token, "src").await.expect("end");
    store
        .forget(&account.token, "src-1")
        .await
        .expect("forget source");
    age(&pool, &person, "src").await;
    push_in(&store, &dev, "trigger", "trig-1", ctx()).await;
    assert_eq!(summary_sources(&pool, &person, "src").await, vec![1]);

    // A summary the person forgot or corrected is not derived again, and an explicit end
    // does not report the hidden memory as success.
    for (session, correct) in [("forgot", false), ("fixed", true)] {
        push_in(&store, &dev, session, &format!("{session}-1"), ctx()).await;
        let summary = store
            .end_session(&account.token, session)
            .await
            .expect("end");
        if correct {
            store
                .correct(
                    &account.token,
                    &summary.memory_id,
                    "corrected text",
                    Vec::new(),
                    ctx(),
                    Default::default(),
                )
                .await
                .expect("correct");
        } else {
            store
                .forget(&account.token, &summary.memory_id)
                .await
                .expect("forget");
        }
        push_in(&store, &dev, session, &format!("{session}-2"), ctx()).await;
        let refused = store.end_session(&account.token, session).await;
        assert!(
            matches!(refused, Err(PostgresError::Validation(ref m)) if m.contains("forgotten or superseded")),
            "{session}: {refused:?}"
        );
        age(&pool, &person, session).await;
        push_in(&store, &dev, "trigger", &format!("trig-{session}"), ctx()).await;
        assert_eq!(
            summary_sources(&pool, &person, session).await,
            vec![1],
            "{session}: not derived again"
        );
    }
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_idle_sweep_reads_only_pending_sessions_and_closes_an_empty_one_quietly() {
    let (pool, store) = pool_and_store().await;
    let account = store
        .bootstrap_account("sweep@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let person = account.account.person_id.clone();
    let dev = device_for(&store, &account, "sweep-dev").await;
    let pending = |session: &'static str| {
        let pool = pool.clone();
        let person = person.clone();
        async move {
            sqlx::query_scalar::<_, Option<bool>>(
                "SELECT pending_since < now() - interval '1 minute' FROM cairn_sessions
                 WHERE person_id = $1 AND session_id = $2",
            )
            .bind(person)
            .bind(session)
            .fetch_one(&pool)
            .await
            .expect("pending row")
        }
    };

    // A closed, summarised session is not pending, so the sweep does not read it.
    push_in(&store, &dev, "done", "done-1", WorkContext::default()).await;
    store
        .end_session(&account.token, "done")
        .await
        .expect("end");
    assert_eq!(pending("done").await, None);

    // A session whose only observation was forgotten has nothing to summarise. The distill job
    // succeeds without a quarantine row and the session leaves the pending index, so the next
    // sweep does not spin.
    push_in(&store, &dev, "bad", "bad-1", WorkContext::default()).await;
    store.forget(&account.token, "bad-1").await.expect("forget");
    sqlx::query("UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes' WHERE person_id = $1 AND session_id = 'bad'")
        .bind(&person)
        .execute(&pool)
        .await
        .expect("age pending");
    assert_eq!(pending("bad").await, Some(true));
    push_in(&store, &dev, "trigger", "sweep-1", WorkContext::default()).await;
    assert_eq!(
        pending("bad").await,
        None,
        "an empty close clears the pending mark"
    );
    assert!(!session_ended(&pool, &person, "bad").await);
    let quarantined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_job_quarantine WHERE kind = 'distill' AND job_key LIKE '%' || $1",
    )
    .bind("bad")
    .fetch_one(&pool)
    .await
    .expect("quarantine count");
    assert_eq!(quarantined, 0);
    let succeeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_jobs WHERE kind = 'distill' AND succeeded AND job_key LIKE '%' || $1",
    )
    .bind("bad")
    .fetch_one(&pool)
    .await
    .expect("succeeded count");
    assert_eq!(succeeded, 1);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_auto_registration_validates_the_repository_length() {
    let (_, store) = pool_and_store().await;
    let account = store
        .bootstrap_account("longrepo@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let dev = device_for(&store, &account, "longrepo-dev").await;
    let context = WorkContext {
        repository: Some("r".repeat(600)),
        ..WorkContext::default()
    };
    push_in(&store, &dev, "long", "long-1", context).await;
    let projects = store.projects(&account.token).await.expect("projects");
    assert!(
        projects
            .iter()
            .all(|project| project.repository.len() <= 512)
    );
    assert!(
        projects.is_empty(),
        "an oversized repository registers nothing"
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_push_waits_for_an_in_flight_cursor_of_the_same_person() {
    let (pool, store) = pool_and_store().await;
    let account = store
        .bootstrap_account("inflight@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let person = account.account.person_id.clone();
    let dev = device_for(&store, &account, "inflight-dev").await;
    // An uncommitted allocation holds the person's cursor lock until commit.
    let mut held = pool.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cairn-sync-cursor:' || $1, 0))")
        .bind(&person)
        .execute(&mut *held)
        .await
        .expect("lock");
    let pusher = tokio::spawn({
        let store = store.clone();
        let dev = dev.clone();
        async move {
            push_in(&store, &dev, "flight", "flight-1", WorkContext::default()).await;
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !pusher.is_finished(),
        "a later cursor must not commit first"
    );
    held.commit().await.expect("commit");
    pusher.await.expect("push finishes");
    let page = store.sync_pull(&dev, 0).await.expect("pull");
    assert_eq!(page.events.len(), 1);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_device_token_is_an_agent_credential_and_web_only_actions_refuse_it() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let password = "correct horse battery staple";
    assert!(!store.has_accounts().await.expect("empty"));
    let owner = store
        .bootstrap_account("dev-owner@example.test", password)
        .await
        .expect("owner");
    assert!(store.has_accounts().await.expect("bootstrapped"));
    let other = store
        .register_account("dev-other@example.test", password)
        .await
        .expect("other");
    let device = device_for(&store, &owner, "agent-device").await;
    let other_device = device_for(&store, &other, "other-device").await;

    // Agent actions run as the owning account and stay inside it.
    let project = store
        .register_project(&device, "acme/devices", "Devices", None)
        .await
        .expect("device registers a project")
        .project_id;
    assert_eq!(store.projects(&owner.token).await.expect("list").len(), 1);
    let foreign = store
        .project_memories(&other_device, &project)
        .await
        .expect_err("another account's device is outside the project");
    assert!(foreign.is_unauthorized() || foreign.to_string().contains("not a project member"));
    let devices = store
        .list_devices(&owner.token)
        .await
        .expect("list devices");
    assert_eq!(devices.len(), 1);
    assert!(
        devices[0].last_seen_at.parse::<u64>().unwrap()
            >= devices[0].registered_at.parse::<u64>().unwrap()
    );

    // Web-only actions refuse a device token with the web-only error.
    let web_only = [
        store.authenticate_session(&device).await.map(|_| ()),
        store.audit_events(&device, 10).await.map(|_| ()),
        store.list_devices(&device).await.map(|_| ()),
        store
            .rotate_device(&device, "agent-device")
            .await
            .map(|_| ()),
        store
            .revoke_device(&device, "agent-device")
            .await
            .map(|_| ()),
        store.logout_account(&device).await,
    ];
    for result in web_only {
        let error = result.expect_err("device token is refused");
        assert!(error.to_string().contains(cairn_core::WEB_ONLY_ERROR));
    }
    assert!(store.is_device_token(&device).await.expect("known"));
    assert!(!store.is_device_token(&owner.token).await.expect("session"));
    assert!(store.authenticate_session(&owner.token).await.is_ok());

    // A device records its personal branches regardless of the project roster.
    let mut merges = Vec::new();
    for name in ["feat/solo", "feat/shared"] {
        let branch = store
            .register_branch(&device, &project, name, None, None, None)
            .await
            .expect("branch");
        let memory = save_project_note(
            &store,
            &device,
            &project,
            &format!("decision on {name}"),
            WorkContext {
                repository: Some("acme/devices".to_owned()),
                branch: Some(name.to_owned()),
                ..WorkContext::default()
            },
        )
        .await;
        merges.push((branch.branch_id, memory.memory_id));
    }
    store
        .confirm_branch_merge(&device, &project, &merges[0].0, &merges[0].1, "c1", "solo")
        .await
        .expect("solo merge with a device token");
    join_project(&store, &other, &project).await;
    store
        .confirm_branch_merge(
            &device,
            &project,
            &merges[1].0,
            &merges[1].1,
            "c2",
            "shared",
        )
        .await
        .expect("device records its own merge in a shared project");

    // A revoked token is rejected on agent actions and on sync.
    store
        .revoke_device(&owner.token, "agent-device")
        .await
        .expect("revoke");
    let revoked = store
        .projects(&device)
        .await
        .expect_err("revoked device token");
    assert!(revoked.is_unauthorized());
    assert!(store.sync_pull(&device, 0).await.is_err());
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_sync_checks_the_account_and_throttles_last_seen() {
    let (pool, store) = pool_and_store().await;
    let account = store
        .bootstrap_account("seen-owner@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let person = account.account.person_id.clone();
    let device = device_for(&store, &account, "seen-device").await;
    let seen = || {
        let (pool, person) = (pool.clone(), person.clone());
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT EXTRACT(EPOCH FROM last_seen_at)::bigint FROM cairn_devices
                 WHERE device_id = 'seen-device' AND person_id = $1",
            )
            .bind(person)
            .fetch_one(&pool)
            .await
            .expect("last seen")
        }
    };
    let set_seen = |interval: &'static str| {
        let (pool, person) = (pool.clone(), person.clone());
        async move {
            sqlx::query(&format!(
                "UPDATE cairn_devices SET last_seen_at = now() - interval '{interval}'
                 WHERE device_id = 'seen-device' AND person_id = $1"
            ))
            .bind(person)
            .execute(&pool)
            .await
            .expect("set last seen");
        }
    };

    // A recent last-seen is left alone by sync pulls and agent requests.
    set_seen("10 seconds").await;
    let before = seen().await;
    store.sync_pull(&device, 0).await.expect("pull");
    store.sync_diagnostics(&device).await.expect("diagnose");
    store.projects(&device).await.expect("agent request");
    assert_eq!(seen().await, before);
    // A stale one is refreshed once.
    set_seen("1 hour").await;
    store.projects(&device).await.expect("agent request");
    assert!(seen().await > before - 60);
    set_seen("1 hour").await;
    store.sync_pull(&device, 0).await.expect("pull");
    assert!(seen().await > before - 60);

    // A disabled account's device is refused on sync as on agent requests.
    sqlx::query("UPDATE cairn_accounts SET status = 'disabled' WHERE person_id = $1")
        .bind(&person)
        .execute(&pool)
        .await
        .expect("disable");
    assert!(store.sync_pull(&device, 0).await.is_err());
    assert!(store.sync_diagnostics(&device).await.is_err());
    assert!(
        store
            .sync_push(
                &device,
                pushed("seen-event", "seen-session", WorkContext::default()),
            )
            .await
            .is_err()
    );
    assert!(store.authenticate_account(&device).await.is_err());
    sqlx::query("UPDATE cairn_accounts SET status = 'active' WHERE person_id = $1")
        .bind(&person)
        .execute(&pool)
        .await
        .expect("enable");
    store.sync_pull(&device, 0).await.expect("pull again");
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_push_adopts_an_observation_that_has_no_sync_event() {
    let database_url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&database_url)
        .await
        .expect("connect and migrate PostgreSQL");
    let account = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .expect("bootstrap account");
    let device = store
        .register_device(
            &account.token,
            "device-adopt",
            &account.account.person_id,
            None,
            "Adopt laptop",
        )
        .await
        .expect("register device");
    let stored = |content: &str| cairn_core::Observation {
        event_id: "evt-adopt".to_owned(),
        session_id: "sess-adopt".to_owned(),
        content: content.to_owned(),
        role: "agent".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: cairn_core::Lifecycle::default(),
        scope: cairn_core::MemoryScope::Personal,
        context: WorkContext::default(),
        captured_at: "0".to_owned(),
    };
    store
        .import_bundle(
            &account.token,
            cairn_core::ExportBundle {
                format: cairn_core::EXPORT_FORMAT.to_owned(),
                version: cairn_core::EXPORT_VERSION,
                exported_at: "0".to_owned(),
                sessions: vec![cairn_core::Session {
                    session_id: "sess-adopt".to_owned(),
                    person_id: None,
                    context: WorkContext::default(),
                    started_at: "0".to_owned(),
                    ended_at: None,
                }],
                observations: vec![stored("adoptable fact")],
                memories: Vec::new(),
                forgotten_ids: Vec::new(),
            },
        )
        .await
        .expect("import an observation with no sync event");
    let raw = PgPool::connect(&database_url).await.expect("fixture pool");
    let count = |table: &'static str| {
        let raw = raw.clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!(
                "SELECT count(*) FROM {table} WHERE event_id = 'evt-adopt'"
            ))
            .fetch_one(&raw)
            .await
            .expect("count rows")
        }
    };
    assert_eq!(count("cairn_sync_events").await, 0);

    // Changed content, session, role, or source is a conflict and writes nothing.
    for changed in [
        pushed("evt-adopt", "sess-adopt", WorkContext::default()),
        ObservationInput {
            content: "adoptable fact".to_owned(),
            session_id: "sess-other".to_owned(),
            ..pushed("evt-adopt", "sess-adopt", WorkContext::default())
        },
        ObservationInput {
            content: "adoptable fact".to_owned(),
            role: "user".to_owned(),
            ..pushed("evt-adopt", "sess-adopt", WorkContext::default())
        },
        ObservationInput {
            content: "adoptable fact".to_owned(),
            source: "other".to_owned(),
            ..pushed("evt-adopt", "sess-adopt", WorkContext::default())
        },
    ] {
        let result = store.sync_push(&device.token, changed).await;
        assert!(
            matches!(result, Err(PostgresError::Conflict(_))),
            "{result:?}"
        );
    }
    assert_eq!(count("cairn_sync_events").await, 0);
    assert_eq!(count("cairn_observations").await, 1);

    // The same event is adopted once: no second row, a sync event, then a duplicate.
    let same = ObservationInput {
        content: "adoptable fact".to_owned(),
        ..pushed("evt-adopt", "sess-adopt", WorkContext::default())
    };
    let adopted = store
        .sync_push(&device.token, same.clone())
        .await
        .expect("adopt the stored observation");
    assert!(adopted.accepted && !adopted.duplicate);
    assert!(adopted.cursor > 0);
    assert_eq!(adopted.observation.content, "adoptable fact");
    assert_eq!(adopted.observation.captured_at, "0");
    assert_eq!(count("cairn_observations").await, 1);
    assert_eq!(count("cairn_sync_events").await, 1);
    let replay = store
        .sync_push(&device.token, same)
        .await
        .expect("replay after adoption");
    assert!(replay.duplicate);
    assert_eq!(replay.cursor, adopted.cursor);
    assert_eq!(count("cairn_sync_events").await, 1);
    let pulled = store.sync_pull(&device.token, 0).await.expect("pull");
    assert_eq!(
        pulled
            .events
            .iter()
            .filter(|event| event.event_id == "evt-adopt")
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_only_the_admin_web_session_creates_accounts() {
    let (_pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let admin = store
        .bootstrap_account("admin-role@example.test", pw)
        .await
        .expect("admin");
    assert!(admin.account.is_admin);
    assert!(
        store
            .authenticate_account(&admin.token)
            .await
            .expect("me")
            .is_admin
    );
    let member = store
        .register_account_as_admin(&admin.token, "member-role@example.test", pw)
        .await
        .expect("admin creates an account");
    assert!(!member.account.is_admin);
    assert!(
        !store
            .login("member-role@example.test", pw)
            .await
            .expect("login")
            .account
            .is_admin
    );
    let error = store
        .register_account_as_admin(&member.token, "x@example.test", pw)
        .await
        .expect_err("non-admin session");
    assert!(error.to_string().contains(cairn_core::ADMIN_ONLY_ERROR));
    let device = store
        .register_device(
            &admin.token,
            "admin-device",
            &admin.account.person_id,
            None,
            "Hook",
        )
        .await
        .expect("device");
    let error = store
        .register_account_as_admin(&device.token, "x@example.test", pw)
        .await
        .expect_err("device token");
    assert!(error.to_string().contains(cairn_core::WEB_ONLY_ERROR));
    assert!(
        store
            .register_account_as_admin("nope", "x@example.test", pw)
            .await
            .expect_err("bad token")
            .is_unauthorized()
    );
    assert!(store.login("x@example.test", pw).await.is_err());
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_equal_scores_rank_decisions_before_other_memories() {
    let (_pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("decision-rank@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let mut decision = String::new();
    for (label, kind) in [
        ("one", cairn_core::MemoryKind::Explicit),
        ("two", cairn_core::MemoryKind::Explicit),
        ("three", cairn_core::MemoryKind::Lesson),
        ("four", cairn_core::MemoryKind::Decision),
        ("five", cairn_core::MemoryKind::Explicit),
        ("six", cairn_core::MemoryKind::Explicit),
    ] {
        let memory = store
            .remember_with_kind(
                &owner.token,
                &format!("rankpgdecision {label}"),
                Vec::new(),
                WorkContext::default(),
                Default::default(),
                kind,
            )
            .await
            .expect("remember");
        if kind == cairn_core::MemoryKind::Decision {
            decision = memory.memory_id;
        }
    }
    let hits = store
        .search(&owner.token, "rankpgdecision", 3, &RecallFilter::default())
        .await
        .expect("search");
    assert_eq!(hits[0].memory_id, decision, "the cap keeps the decision");
    let pack = store
        .context(&owner.token, "rankpgdecision", 10, &RecallFilter::default())
        .await
        .expect("context");
    assert_eq!(pack.results[0].memory_id, decision);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_periodic_sweep_closes_idle_sessions_for_every_person() {
    let (pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let a = store
        .bootstrap_account("sweep-a@example.test", pw)
        .await
        .expect("a");
    let b = store
        .register_account("sweep-b@example.test", pw)
        .await
        .expect("b");
    let (a_id, b_id) = (a.account.person_id.clone(), b.account.person_id.clone());
    let (dev_a, dev_b) = (
        device_for(&store, &a, "sweep-a").await,
        device_for(&store, &b, "sweep-b").await,
    );
    push_in(&store, &dev_a, "sweep-a", "sa-1", WorkContext::default()).await;
    push_in(&store, &dev_a, "fresh-a", "fa-1", WorkContext::default()).await;
    push_in(&store, &dev_b, "sweep-b", "sb-1", WorkContext::default()).await;
    age(&pool, &a_id, "sweep-a").await;
    age(&pool, &b_id, "sweep-b").await;
    assert!(!session_ended(&pool, &a_id, "sweep-a").await);

    store.sweep_idle_sessions().await;

    assert!(session_ended(&pool, &a_id, "sweep-a").await);
    assert!(session_ended(&pool, &b_id, "sweep-b").await);
    assert_eq!(summary_sources(&pool, &a_id, "sweep-a").await, vec![1]);
    assert_eq!(summary_sources(&pool, &b_id, "sweep-b").await, vec![1]);
    assert!(!session_ended(&pool, &a_id, "fresh-a").await, "not idle");
    store.sweep_idle_sessions().await;
    assert_eq!(summary_sources(&pool, &a_id, "sweep-a").await, vec![1]);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_push_gives_the_session_its_context_so_the_summary_is_scoped() {
    let (pool, store) = pool_and_store().await;
    let a = store
        .bootstrap_account("push-scope@example.test", "correct horse battery staple")
        .await
        .expect("account");
    let a_id = a.account.person_id.clone();
    let device = device_for(&store, &a, "push-scope").await;
    let hook = |branch: &str| WorkContext {
        repository: Some("cairn".to_owned()),
        branch: Some(branch.to_owned()),
        ..WorkContext::default()
    };
    push_in(&store, &device, "created", "pc-1", hook("feat/hook")).await;
    // A push into an empty row fills it; a later push never replaces a real context.
    push_in(&store, &device, "filled", "pf-1", WorkContext::default()).await;
    push_in(&store, &device, "filled", "pf-2", hook("feat/hook")).await;
    push_in(&store, &device, "filled", "pf-3", hook("feat/other")).await;
    for session in ["created", "filled"] {
        age(&pool, &a_id, session).await;
    }

    store.sweep_idle_sessions().await;

    let filter = RecallFilter {
        context: Some(hook("feat/hook")),
        ..Default::default()
    };
    let hits = store
        .search(&a.token, "autofact", 10, &filter)
        .await
        .expect("filtered recall");
    for session in ["created", "filled"] {
        let summary_id = format!("memory-{a_id}-{session}");
        let scope: String = sqlx::query_scalar(
            "SELECT scope FROM cairn_memories WHERE person_id = $1 AND memory_id = $2",
        )
        .bind(&a_id)
        .bind(&summary_id)
        .fetch_one(&pool)
        .await
        .expect("summary row");
        assert_eq!(scope, "personal", "{session}");
        assert!(
            hits.iter().any(|hit| hit.memory_id == summary_id),
            "a repository/branch-filtered recall finds the {session} summary"
        );
    }
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_init_project_creates_once_and_defaults_the_name() {
    let (_pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let owner = store
        .bootstrap_account("init-owner@example.test", pw)
        .await
        .expect("owner");
    let other = store
        .register_account("init-other@example.test", pw)
        .await
        .expect("other");
    let (created, is_new) = store
        .init_project(&owner.token, " repo-a ", None)
        .await
        .expect("init");
    assert!(is_new);
    assert_eq!(
        (created.repository.as_str(), created.name.as_str()),
        ("repo-a", "repo-a")
    );
    assert!(
        store
            .init_project(&owner.token, "repo-a", Some("Renamed"))
            .await
            .is_err()
    );
    let (again, is_new) = store
        .init_project(&owner.token, "repo-a", Some("repo-a"))
        .await
        .expect("again");
    assert!(!is_new);
    assert_eq!(again.project_id, created.project_id);
    let (theirs, is_new) = store
        .init_project(&other.token, "repo-a", Some("Named"))
        .await
        .expect("other init");
    assert!(is_new);
    assert_ne!(theirs.project_id, created.project_id);
    assert_eq!(theirs.name, "Named");
    assert!(store.init_project(&owner.token, "  ", None).await.is_err());
    assert!(
        store
            .init_project(&owner.token, &"r".repeat(513), None)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_admin_lists_accounts_and_passwords_reset_and_change_revoke_sessions() {
    let (pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let fresh = "a brand new passphrase";
    let admin = store
        .bootstrap_account("pw-admin@example.test", pw)
        .await
        .expect("admin");
    let user = store
        .register_account("pw-user@example.test", pw)
        .await
        .expect("user");
    let second = store
        .login("pw-user@example.test", pw)
        .await
        .expect("second session");
    let device = device_for(&store, &user, "pw-device").await;

    // Listing is admin web session only.
    let listed = store.accounts(&admin.token).await.expect("list");
    assert_eq!(
        listed.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
        ["pw-admin@example.test", "pw-user@example.test"]
    );
    assert!(listed[0].is_admin && !listed[1].is_admin);
    for (token, expected) in [
        (&user.token, cairn_core::ADMIN_ONLY_ERROR),
        (&device, cairn_core::WEB_ONLY_ERROR),
    ] {
        let refused = store.accounts(token).await.expect_err("refused");
        assert!(refused.to_string().contains(expected), "{refused}");
        let refused = store
            .reset_account_password(token, &user.account.person_id, fresh)
            .await
            .expect_err("refused");
        assert!(refused.to_string().contains(expected), "{refused}");
    }
    assert!(
        store
            .reset_account_password(&admin.token, &user.account.person_id, "short")
            .await
            .is_err()
    );
    assert!(
        store
            .reset_account_password(&admin.token, "person-missing", fresh)
            .await
            .is_err()
    );
    store
        .authenticate_session(&user.token)
        .await
        .expect("untouched");

    // The reset ends every web session of that user, keeps devices, and is audited.
    store
        .reset_account_password(&admin.token, &user.account.person_id, fresh)
        .await
        .expect("reset");
    for token in [&user.token, &second.token] {
        assert!(store.authenticate_session(token).await.is_err());
    }
    store
        .authenticate_account(&device)
        .await
        .expect("device stays valid");
    store
        .authenticate_session(&admin.token)
        .await
        .expect("admin session stays");
    assert!(store.login("pw-user@example.test", pw).await.is_err());
    let relogin = store
        .login("pw-user@example.test", fresh)
        .await
        .expect("new password");
    let audit = store
        .audit_events(&relogin.token, 100)
        .await
        .expect("audit");
    assert!(
        audit
            .iter()
            .any(|event| event.action == "account_password_reset")
    );
    let (leaked,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM cairn_audit_events WHERE metadata::text LIKE $1")
            .bind(format!("%{fresh}%"))
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(leaked, 0);

    // A change needs the current password and keeps only the current session.
    let other = store
        .login("pw-user@example.test", fresh)
        .await
        .expect("other session");
    let wrong = store
        .change_password(&relogin.token, "not the password", pw)
        .await
        .expect_err("wrong current password");
    assert!(wrong.is_unauthorized(), "{wrong}");
    assert!(
        store
            .change_password(&relogin.token, fresh, "short")
            .await
            .is_err()
    );
    assert!(
        store
            .change_password(&device, fresh, pw)
            .await
            .expect_err("device")
            .to_string()
            .contains(cairn_core::WEB_ONLY_ERROR)
    );
    store
        .authenticate_session(&other.token)
        .await
        .expect("still valid");
    store
        .change_password(&relogin.token, fresh, pw)
        .await
        .expect("change");
    store
        .authenticate_session(&relogin.token)
        .await
        .expect("current kept");
    assert!(store.authenticate_session(&other.token).await.is_err());
    store
        .authenticate_account(&device)
        .await
        .expect("device stays valid");
    assert!(store.login("pw-user@example.test", fresh).await.is_err());
    store
        .login("pw-user@example.test", pw)
        .await
        .expect("changed password");
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_personal_capture_attachment_and_branch_records() {
    let (_, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let owner = store
        .bootstrap_account("owner@example.test", pw)
        .await
        .unwrap();
    let member = store
        .register_account("member@example.test", pw)
        .await
        .unwrap();
    let dev = device_for(&store, &member, "personal-device").await;
    let input = ObservationInput {
        event_id: "personal".into(),
        session_id: "s".into(),
        content: "personal note".into(),
        role: "agent".into(),
        source: "test".into(),
        importance: None,
        lifecycle: Default::default(),
        context: WorkContext {
            repository: Some("repo".into()),
            ..WorkContext::default()
        },
    };
    let capture = store.sync_push(&dev, input.clone()).await.unwrap();
    assert_eq!(capture.project_attachment.unwrap().status, "personal");
    assert!(store.projects(&member.token).await.unwrap().is_empty());
    let mut broken = input.clone();
    broken.event_id = "unknown".into();
    broken.context.project_id = Some("project-unknown".into());
    let result = store.sync_push(&dev, broken).await.unwrap();
    assert!(result.project_attachment.unwrap().warning.is_some());
    assert!(store.projects(&member.token).await.unwrap().is_empty());
    let project = store
        .register_project(&owner.token, "repo", "Repo", None)
        .await
        .unwrap();
    let mut attached = input;
    attached.event_id = "attached".into();
    attached.context.project_id = Some(project.project_id.clone());
    attached.context.branch = Some("feature".into());
    assert_eq!(
        store
            .sync_push(&dev, attached.clone())
            .await
            .unwrap()
            .project_attachment
            .unwrap()
            .status,
        "attached"
    );
    assert!(store.sync_push(&dev, attached).await.unwrap().duplicate);
    assert_eq!(
        store
            .project_people(&owner.token, &project.project_id)
            .await
            .unwrap()
            .len(),
        2
    );
    let member_branch = store
        .branches(&member.token, &project.project_id)
        .await
        .unwrap();
    assert_eq!(member_branch.len(), 1);
    assert!(
        store
            .branches(&owner.token, &project.project_id)
            .await
            .unwrap()
            .is_empty()
    );
    let source = save_project_note(
        &store,
        &member.token,
        &project.project_id,
        "branch evidence",
        WorkContext {
            repository: Some("repo".into()),
            branch: Some("feature".into()),
            ..WorkContext::default()
        },
    )
    .await;
    let merged = store
        .confirm_branch_merge(
            &dev,
            &project.project_id,
            &member_branch[0].branch_id,
            &source.memory_id,
            "mc",
            "record merge",
        )
        .await
        .unwrap();
    assert_eq!(merged.branch.status, "merged");
    assert!(
        store
            .close_branch(
                &owner.token,
                &project.project_id,
                &member_branch[0].branch_id,
                "not mine"
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_rename_is_owner_only_audited_and_checks_name_before_join() {
    let (pool, store) = pool_and_store().await;
    let pw = "correct horse battery staple";
    let owner = store
        .bootstrap_account("rename-owner@example.test", pw)
        .await
        .unwrap();
    let member = store
        .register_account("rename-member@example.test", pw)
        .await
        .unwrap();
    let outsider = store
        .register_account("rename-outsider@example.test", pw)
        .await
        .unwrap();
    let (project, _) = store
        .init_project(&owner.token, "repo", Some("--help"))
        .await
        .unwrap();
    assert!(
        store
            .join_project_named(&member.token, &project.project_id, Some("Wrong"))
            .await
            .is_err()
    );
    assert!(store.projects(&member.token).await.unwrap().is_empty());
    // Name validation and membership must use the same transaction/connection.
    let single_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let single = PostgresStore::from_pool(single_pool);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        single.join_project_named(&member.token, &project.project_id, Some("--help")),
    )
    .await
    .unwrap()
    .unwrap();
    for token in [&member.token, &outsider.token] {
        assert!(
            store
                .rename_project(token, &project.project_id, "No")
                .await
                .is_err()
        );
    }
    for name in [" ".to_owned(), "x".repeat(513)] {
        assert!(
            store
                .rename_project(&owner.token, &project.project_id, &name)
                .await
                .is_err()
        );
    }
    let renamed = store
        .rename_project(&owner.token, &project.project_id, "  Useful name  ")
        .await
        .unwrap();
    assert_eq!(renamed.name, "Useful name");
    assert_eq!(renamed.project_id, project.project_id);
    store
        .rename_project(&owner.token, &project.project_id, "Useful name")
        .await
        .unwrap();
    let audits = store.audit_events(&owner.token, 100).await.unwrap();
    let renames: Vec<_> = audits
        .iter()
        .filter(|event| event.action == "project_renamed")
        .collect();
    assert_eq!(renames.len(), 1);
    assert_eq!(
        renames[0].subject_id.as_deref(),
        Some(project.project_id.as_str())
    );
    assert_eq!(
        renames[0].metadata,
        serde_json::json!({"previous_name":"--help", "name":"Useful name"})
    );
    let persisted: String =
        sqlx::query_scalar("SELECT name FROM cairn_projects WHERE project_id = $1")
            .bind(&project.project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, "Useful name");
    assert_eq!(
        store.projects(&member.token).await.unwrap()[0].name,
        "Useful name"
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_structured_save_requires_fields_and_round_trips_types() {
    let store = PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let admin = store
        .bootstrap_account("admin@shape.test", "correct horse battery staple")
        .await
        .unwrap();
    for field in ["scope", "title", "summary", "reason"] {
        let mut input = serde_json::json!({"scope":"personal", "title":"Title", "summary":"Summary", "reason":"Reason"});
        input.as_object_mut().unwrap().remove(field);
        let error = store
            .save(&admin.token, serde_json::from_value(input).unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(field), "{error}");
    }
    for (kind, fields) in [
        (
            "decision",
            serde_json::json!({"decision_context":"constraints", "alternatives":["other"], "consequences":"tradeoff"}),
        ),
        (
            "rule",
            serde_json::json!({"rule":"borrow", "exceptions":"owned transfer"}),
        ),
        (
            "gotcha",
            serde_json::json!({"symptom":"failure", "cause":"race", "fix":"lock"}),
        ),
        (
            "procedure",
            serde_json::json!({"steps":["first","second"], "when":"release"}),
        ),
        (
            "fact",
            serde_json::json!({"statement":"verified", "source":"check"}),
        ),
        (
            "preference",
            serde_json::json!({"preference":"concise", "example":"one paragraph"}),
        ),
    ] {
        let mut input = serde_json::json!({"scope":"personal", "title":"Title", "summary":"Summary", "reason":"Reason", "kind":kind, "details":"Body", "applies_to":["src/**"], "evidence":["cargo test"], "tags":["rust"], "entities":["Memory"], "pinned":true, "importance":90, "status":"active"});
        input
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let memory = store
            .save(&admin.token, serde_json::from_value(input.clone()).unwrap())
            .await
            .unwrap();
        let found = store
            .memories(&admin.token)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.memory_id == memory.memory_id)
            .unwrap();
        let output = serde_json::to_value(found).unwrap();
        for (key, value) in input.as_object().unwrap() {
            assert_eq!(&output[key], value, "{kind}.{key}");
        }
    }
}

async fn save_project_note(
    store: &PostgresStore,
    token: &str,
    project_id: &str,
    title: &str,
    context: WorkContext,
) -> cairn_core::Memory {
    store.save(token, serde_json::from_value(serde_json::json!({"scope":"project","kind":"decision","title":title,"summary":title,"reason":"Verified fixture","context":WorkContext {project_id:Some(project_id.to_owned()), ..context}})).unwrap()).await.unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_record_title_has_highest_lexical_weight() {
    let store = PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let account = store
        .bootstrap_account("rank@shape.test", "correct horse battery staple")
        .await
        .unwrap();
    for (title, summary) in [("Ordinary", "weightneedle"), ("weightneedle", "Ordinary")] {
        store.save(&account.token,serde_json::from_value(serde_json::json!({"scope":"personal","title":title,"summary":summary,"reason":"verified"})).unwrap()).await.unwrap();
    }
    let rows = store
        .search(&account.token, "weightneedle", 2, &RecallFilter::default())
        .await
        .unwrap();
    assert_eq!(rows[0].fields.title, "weightneedle");
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_hybrid_entities_links_vectors_and_visibility() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM cairn_schema_migrations WHERE version='0014_hybrid_embeddings')"
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    let owner = store
        .bootstrap_account("hybrid@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account_as_admin(
            &owner.token,
            "other@hybrid.test",
            "correct horse battery staple",
        )
        .await
        .unwrap();
    let seed=store.save(&owner.token,serde_json::from_value(serde_json::json!({"scope":"personal","title":"Orchard cultivation","summary":"Grow apples","reason":"food","tags":["planting"],"entities":["Tree::grow"]})).unwrap()).await.unwrap();
    let neighbour=store.save(&owner.token,serde_json::from_value(serde_json::json!({"scope":"personal","title":"Seasonal advice","summary":"Water regularly","reason":"health","evidence":[seed.memory_id]})).unwrap()).await.unwrap();
    let hidden=store.save(&other.token,serde_json::from_value(serde_json::json!({"scope":"personal","title":"Secret horticulture","summary":"Keep private","reason":"privacy","evidence":[seed.memory_id]})).unwrap()).await.unwrap();
    for query in ["planting", "Tree::grow", "Orchard"] {
        let hits = store
            .search(&owner.token, query, 10, &RecallFilter::default())
            .await
            .unwrap();
        assert!(hits.iter().any(|r| r.memory_id == seed.memory_id));
        assert!(hits.iter().any(|r| r.memory_id == neighbour.memory_id));
        assert!(!hits.iter().any(|r| r.memory_id == hidden.memory_id));
    }
    let mut vector = vec![0.0; 384];
    vector[0] = 1.0;
    for m in [&seed, &hidden] {
        assert!(
            store
                .put_embedding(cairn_core::MemoryEmbedding::new(m, vector.clone()).unwrap())
                .await
                .unwrap()
        );
    }
    let hits = store
        .search_with_vector(
            &owner.token,
            "unrelated",
            10,
            &RecallFilter::default(),
            Some(&vector),
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|r| r.memory_id == seed.memory_id));
    assert!(hits.iter().any(|r| r.memory_id == neighbour.memory_id));
    assert!(!hits.iter().any(|r| r.memory_id == hidden.memory_id));
    assert!(hits.iter().all(|r| r.source == "personal"));
    sqlx::query("UPDATE cairn_memories SET record=jsonb_set(record,'{title}','\"Updated title\"') WHERE memory_id=$1").bind(&seed.memory_id).execute(&pool).await.unwrap();
    assert!(
        store
            .search_with_vector(
                &owner.token,
                "unrelated",
                10,
                &RecallFilter::default(),
                Some(&vector)
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .put_embedding(cairn_core::MemoryEmbedding::new(&seed, vector.clone()).unwrap())
            .await
            .unwrap()
    );
    let current = store
        .embedding_jobs(512)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.memory_id == seed.memory_id)
        .unwrap();
    assert!(
        store
            .put_embedding(cairn_core::MemoryEmbedding::new(&current, vector.clone()).unwrap())
            .await
            .unwrap()
    );
    store.forget(&owner.token, &seed.memory_id).await.unwrap();
    assert!(
        store
            .search_with_vector(
                &owner.token,
                "unrelated",
                10,
                &RecallFilter::default(),
                Some(&vector)
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .put_embedding(cairn_core::MemoryEmbedding::new(&seed, vector).unwrap())
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_retention_and_feedback_match_the_file_store() {
    let database_url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&database_url).await.unwrap();
    let owner = store
        .bootstrap_account("retain@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account("retain-other@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let capture = |title: &str, kind: &str| {
        serde_json::json!({
            "scope": "personal",
            "kind": kind,
            "title": title,
            "summary": title,
            "reason": "verified"
        })
    };
    let fresh = store
        .save(
            &owner.token,
            serde_json::from_value(capture("decaytoken shared a", "capture")).unwrap(),
        )
        .await
        .unwrap();
    let old = store
        .save(
            &owner.token,
            serde_json::from_value(capture("decaytoken shared b", "capture")).unwrap(),
        )
        .await
        .unwrap();
    let pool = PgPool::connect(&database_url).await.unwrap();
    let aged = (cairn_core::now_seconds() - 120 * 86_400) as f64;
    sqlx::query("UPDATE cairn_memories SET created_at = to_timestamp($1), lifecycle = jsonb_set(lifecycle, '{importance}', '50'::jsonb) WHERE memory_id = $2")
        .bind(aged)
        .bind(&old.memory_id)
        .execute(&pool)
        .await
        .unwrap();
    let file_path = std::env::temp_dir().join(format!(
        "cairn-retention-parity-{}.json",
        cairn_core::random_token("test")
    ));
    let mut file = cairn_core::MemoryStore::open(&file_path).unwrap();
    let file_owner = file
        .bootstrap_account("retain@example.test", "correct horse battery staple")
        .unwrap();
    file.import_for_account(
        &file_owner.token,
        store.export_bundle(&owner.token).await.unwrap(),
    )
    .unwrap();
    let before = store.memories(&owner.token).await.unwrap().len();
    let hits = store
        .search(&owner.token, "decaytoken", 10, &RecallFilter::default())
        .await
        .unwrap();
    assert_eq!(store.memories(&owner.token).await.unwrap().len(), before);
    assert_eq!(hits.len(), 2);
    let file_hits = file
        .search_for_account(
            &file_owner.token,
            "decaytoken",
            10,
            &RecallFilter::default(),
        )
        .unwrap();
    assert_eq!(hits.len(), file_hits.len());
    for (pg, local) in hits.iter().zip(&file_hits) {
        assert_eq!(pg.memory_id, local.memory_id);
        assert!(
            (pg.score - local.score).abs() < 1e-5,
            "{} / {}",
            pg.score,
            local.score
        );
    }
    let fresh_hit = hits
        .iter()
        .find(|hit| hit.memory_id == fresh.memory_id)
        .unwrap();
    let old_hit = hits
        .iter()
        .find(|hit| hit.memory_id == old.memory_id)
        .unwrap();
    assert!(fresh_hit.score > old_hit.score);
    let limited = store
        .search(&owner.token, "decaytoken", 1, &RecallFilter::default())
        .await
        .unwrap();
    let file_limited = file
        .search_for_account(&file_owner.token, "decaytoken", 1, &RecallFilter::default())
        .unwrap();
    assert_eq!(limited[0].memory_id, file_limited[0].memory_id);
    let counts = store.memories(&owner.token).await.unwrap();
    for pg in &counts {
        let local = file
            .memories_for_account(&file_owner.token)
            .unwrap()
            .into_iter()
            .find(|m| m.memory_id == pg.memory_id)
            .unwrap();
        assert_eq!(pg.access_count, local.access_count);
        assert_eq!(
            pg.last_accessed_at.is_some(),
            local.last_accessed_at.is_some()
        );
    }
    let top = counts
        .iter()
        .find(|memory| memory.memory_id == limited[0].memory_id)
        .unwrap();
    let rest = counts
        .iter()
        .find(|memory| {
            memory.memory_id != limited[0].memory_id
                && (memory.memory_id == fresh.memory_id || memory.memory_id == old.memory_id)
        })
        .unwrap();
    assert_eq!(top.access_count, 2);
    assert_eq!(rest.access_count, 1);
    let pack = store
        .context(&owner.token, "decaytoken", 1, &RecallFilter::default())
        .await
        .unwrap();
    assert_eq!(pack.omitted_count, 1);
    let omitted_id = &pack.omitted_memory_ids[0];
    let expected = counts
        .iter()
        .find(|memory| &memory.memory_id == omitted_id)
        .unwrap()
        .access_count;
    assert_eq!(
        counts_after(&store, &owner.token, omitted_id).await,
        expected
    );

    let personal = store
        .save(
            &owner.token,
            serde_json::from_value(capture("privatepref", "preference")).unwrap(),
        )
        .await
        .unwrap();
    let refused = store
        .feedback(&other.token, &personal.memory_id, "stale")
        .await
        .expect_err("owner only");
    assert!(refused.to_string().contains("unknown memory"));
    let project = store
        .register_project(&owner.token, "retain-repo", "Retain", None)
        .await
        .unwrap();
    let shared = store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "project",
                "kind": "fact",
                "title": "sharedtoken",
                "summary": "sharedtoken",
                "reason": "verified",
                "statement": "holds",
                "context": {"project_id": project.project_id}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        store
            .feedback(&other.token, &shared.memory_id, "helpful")
            .await
            .is_err()
    );
    join_project(&store, &other, &project.project_id).await;
    let before_vote = store
        .search(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .await
        .unwrap();
    let voted = store
        .feedback(&other.token, &shared.memory_id, "helpful")
        .await
        .unwrap();
    assert_eq!(voted.helpful_count, 1);
    let owner_view = store
        .search(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .await
        .unwrap();
    assert_eq!(owner_view.len(), 1);
    assert!(owner_view[0].score > before_vote[0].score);
    let duplicate = store
        .feedback(&other.token, &shared.memory_id, "helpful")
        .await
        .unwrap();
    assert_eq!(
        (
            duplicate.helpful_count,
            duplicate.stale_count,
            duplicate.feedback.len()
        ),
        (1, 0, 1)
    );
    let stale = store
        .feedback(&other.token, &shared.memory_id, "stale")
        .await
        .unwrap();
    assert_eq!(
        (stale.helpful_count, stale.stale_count, stale.feedback.len()),
        (0, 1, 1)
    );
    assert_eq!(stale.feedback[0].vote, cairn_core::FeedbackVote::Stale);
    let helpful = store
        .feedback(&other.token, &shared.memory_id, "helpful")
        .await
        .unwrap();
    assert_eq!(
        (
            helpful.helpful_count,
            helpful.stale_count,
            helpful.feedback.len()
        ),
        (1, 0, 1)
    );
    let global = store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "global",
                "kind": "fact",
                "title": "globaltoken",
                "summary": "globaltoken",
                "reason": "verified",
                "statement": "holds"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let global_vote = store
        .feedback(&other.token, &global.memory_id, "stale")
        .await
        .unwrap();
    assert_eq!(global_vote.stale_count, 1);
    assert!(
        store
            .feedback(&owner.token, &global.memory_id, "maybe")
            .await
            .is_err()
    );
    let rule = store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "personal",
                "kind": "rule",
                "title": "ruletoken shared",
                "summary": "ruletoken shared",
                "reason": "verified",
                "rule": "keep"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let rule_fresh = store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "personal",
                "kind": "rule",
                "title": "ruletoken shared",
                "summary": "ruletoken shared",
                "reason": "verified",
                "rule": "keep"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE cairn_memories SET created_at = to_timestamp($1) WHERE memory_id = $2")
        .bind((cairn_core::now_seconds() - 400 * 86_400) as f64)
        .bind(&rule.memory_id)
        .execute(&pool)
        .await
        .unwrap();
    let rules = store
        .search(&owner.token, "ruletoken", 10, &RecallFilter::default())
        .await
        .unwrap();
    let high = rules.iter().map(|hit| hit.score).fold(0.0_f32, f32::max);
    let low = rules.iter().map(|hit| hit.score).fold(f32::MAX, f32::min);
    assert!(high / low < 1.2, "{high} / {low}");
    assert!(
        rules
            .iter()
            .any(|hit| hit.memory_id == rule_fresh.memory_id)
    );
    for vote in ["helpful", "helpful", "stale", "helpful"] {
        let pg = store
            .feedback(&owner.token, &fresh.memory_id, vote)
            .await
            .unwrap();
        let local = file
            .feedback_for_account(&file_owner.token, &fresh.memory_id, vote)
            .unwrap();
        assert_eq!(
            (pg.helpful_count, pg.stale_count),
            (local.helpful_count, local.stale_count)
        );
        assert_eq!(pg.feedback.len(), local.feedback.len());
        assert_eq!(pg.feedback[0].vote, local.feedback[0].vote);
    }
    std::fs::remove_file(file_path).unwrap();
    pool.close().await;
}

async fn counts_after(store: &PostgresStore, token: &str, memory_id: &str) -> u64 {
    store
        .memories(token)
        .await
        .unwrap()
        .into_iter()
        .find(|memory| memory.memory_id == memory_id)
        .unwrap()
        .access_count
}

fn feedback_test_input(scope: &str, title: &str) -> cairn_core::SaveMemoryInput {
    serde_json::from_value(serde_json::json!({
        "scope": scope, "kind": "fact", "title": title,
        "summary": title, "reason": "verified", "statement": "holds"
    }))
    .unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_feedback_rejects_every_memory_hidden_from_reads() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .await
        .unwrap();
    for hidden in [
        "forgotten",
        "source-forgotten",
        "superseded",
        "expired",
        "successor",
    ] {
        let saved = store
            .save(&owner.token, feedback_test_input("global", hidden))
            .await
            .unwrap();
        match hidden {
            "forgotten" => store.forget(&owner.token, &saved.memory_id).await.unwrap(),
            "source-forgotten" => {
                sqlx::query(
                    "INSERT INTO cairn_forgotten_items (person_id, item_id) VALUES ($1, $2)",
                )
                .bind(&owner.account.person_id)
                .bind(&saved.source_event_ids[0])
                .execute(&pool)
                .await
                .unwrap();
            }
            "superseded" => {
                sqlx::query("UPDATE cairn_memories SET record=jsonb_set(record, '{status}', '\"superseded\"'::jsonb) WHERE memory_id=$1")
                    .bind(&saved.memory_id).execute(&pool).await.unwrap();
            }
            "expired" => {
                sqlx::query("UPDATE cairn_memories SET lifecycle=jsonb_set(lifecycle, '{expiresAt}', '\"1\"'::jsonb) WHERE memory_id=$1")
                    .bind(&saved.memory_id).execute(&pool).await.unwrap();
            }
            "successor" => {
                let successor = store
                    .save(&owner.token, feedback_test_input("global", "successor"))
                    .await
                    .unwrap();
                sqlx::query("UPDATE cairn_memories SET supersedes=$1 WHERE memory_id=$2")
                    .bind(&saved.memory_id)
                    .bind(&successor.memory_id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            !store
                .memories(&other.token)
                .await
                .unwrap()
                .iter()
                .any(|m| m.memory_id == saved.memory_id),
            "{hidden}"
        );
        for token in [&owner.token, &other.token] {
            let error = store
                .feedback(token, &saved.memory_id, "helpful")
                .await
                .expect_err(hidden);
            assert_eq!(
                error.to_string(),
                format!("validation error: unknown memory: {}", saved.memory_id)
            );
        }
        let votes: serde_json::Value =
            sqlx::query_scalar("SELECT feedback FROM cairn_memories WHERE memory_id=$1")
                .bind(&saved.memory_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(votes, serde_json::json!([]));
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_concurrent_feedback_preserves_every_persons_vote() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let memory = store
        .save(
            &owner.token,
            feedback_test_input("global", "concurrentneedle"),
        )
        .await
        .unwrap();
    let mut guard = pool.begin().await.unwrap();
    sqlx::query("SELECT memory_id FROM cairn_memories WHERE memory_id=$1 FOR UPDATE")
        .bind(&memory.memory_id)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    let mut votes = Vec::new();
    for token in [&owner.token, &other.token] {
        let store = store.clone();
        let token = token.clone();
        let id = memory.memory_id.clone();
        votes.push(tokio::spawn(async move {
            store.feedback(&token, &id, "helpful").await
        }));
    }
    // Both requests must reach the locked row before either can write its vote.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%cairn_memories%' AND query LIKE '%feedback%'")
                .fetch_one(&pool).await.unwrap();
            if blocked == 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("both feedback requests blocked on the held row lock");
    guard.commit().await.unwrap();
    for vote in votes {
        vote.await.unwrap().unwrap();
    }
    let saved = store
        .memories(&owner.token)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.memory_id == memory.memory_id)
        .unwrap();
    assert_eq!(
        (saved.helpful_count, saved.stale_count, saved.feedback.len()),
        (2, 0, 2)
    );
    for person in [&owner.account.person_id, &other.account.person_id] {
        assert_eq!(
            saved
                .feedback
                .iter()
                .filter(|vote| &vote.person_id == person)
                .count(),
            1
        );
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_summary_regeneration_preserves_reinforcement_and_feedback() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = store
        .register_device(
            &owner.token,
            "summary-feedback-device",
            &owner.account.person_id,
            None,
            "test",
        )
        .await
        .unwrap();
    store
        .start_session(&owner.token, "summary-feedback", WorkContext::default())
        .await
        .unwrap();
    let observation = |event: &str| ObservationInput {
        event_id: event.to_owned(),
        session_id: "summary-feedback".to_owned(),
        content: "summaryneedle capture".to_owned(),
        role: "user".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: Default::default(),
        context: WorkContext::default(),
    };
    store
        .sync_push(&device.token, observation("first"))
        .await
        .unwrap();
    let summary = store
        .end_session(&owner.token, "summary-feedback")
        .await
        .unwrap();
    for (event, vote) in [("second", "stale"), ("third", "helpful")] {
        store
            .feedback(&owner.token, &summary.memory_id, vote)
            .await
            .unwrap();
        store
            .search(&owner.token, "summaryneedle", 20, &RecallFilter::default())
            .await
            .unwrap();
        let before = store
            .memories(&owner.token)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.memory_id == summary.memory_id)
            .unwrap();
        assert!(before.access_count > 0);
        assert!(before.last_accessed_at.is_some());
        assert_eq!(before.feedback.len(), 1);
        store
            .sync_push(&device.token, observation(event))
            .await
            .unwrap();
        let after = store
            .end_session(&owner.token, "summary-feedback")
            .await
            .unwrap();
        assert_eq!(after.memory_id, before.memory_id);
        assert!(after.source_event_ids.len() > before.source_event_ids.len());
        assert_eq!(after.access_count, before.access_count);
        assert_eq!(after.last_accessed_at, before.last_accessed_at);
        assert_eq!(after.feedback, before.feedback);
        assert_eq!(after.helpful_count, before.helpful_count);
        assert_eq!(after.stale_count, before.stale_count);
        let stored = store
            .memories(&owner.token)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.memory_id == summary.memory_id)
            .unwrap();
        assert_eq!(stored, after);
    }
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_import_discards_foreign_votes_and_untrusted_counts() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let saved = store
        .save(
            &owner.token,
            feedback_test_input("personal", "importneedle"),
        )
        .await
        .unwrap();
    let mut bundle = store.export_bundle(&owner.token).await.unwrap();
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
                    cairn_core::MemoryFeedback {
                        person_id: other.account.person_id.clone(),
                        vote: cairn_core::FeedbackVote::Helpful
                    };
                    3
                ];
            }
            memory
        })
        .collect();
    assert_eq!(
        store
            .import_bundle(&owner.token, bundle.clone())
            .await
            .unwrap()
            .imported_memories,
        2
    );
    for memory in store
        .memories(&owner.token)
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.memory_id == "forged-votes" || m.memory_id == "counts-only")
    {
        assert!(memory.feedback.is_empty(), "{}", memory.memory_id);
        assert_eq!((memory.helpful_count, memory.stale_count), (0, 0));
        let error = store
            .feedback(&other.token, &memory.memory_id, "helpful")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown memory"));
    }
    let counts: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT helpful_count, stale_count FROM cairn_memories WHERE memory_id IN ('forged-votes', 'counts-only')",
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(counts, vec![(0, 0); 2]);
    assert_eq!(
        store
            .import_bundle(&owner.token, bundle)
            .await
            .unwrap()
            .imported_memories,
        0
    );
    pool.close().await;
}

fn review_note(session_id: &str, event_id: &str) -> ObservationInput {
    ObservationInput {
        event_id: event_id.to_owned(),
        session_id: session_id.to_owned(),
        content: "Decision: retain this capture for the summary.".to_owned(),
        role: "assistant".to_owned(),
        source: "test".to_owned(),
        importance: Some("high".to_owned()),
        lifecycle: Default::default(),
        context: WorkContext::default(),
    }
}

async fn open_pool() -> PgPool {
    PgPool::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_retry_with_a_pending_successor_keeps_one_row_and_the_tick_continues() {
    let (_pool, store) = pool_and_store().await;
    let pool = open_pool().await;
    let now = 1_700_000_000_u64;
    store
        .enqueue_job(cairn_core::JobKind::Embed, "duplicate-key", now, true)
        .await
        .unwrap();
    let _claimed = store
        .poll_jobs(cairn_core::JobKind::Embed, 1, now)
        .await
        .unwrap()
        .remove(0);
    store
        .enqueue_job(cairn_core::JobKind::Embed, "duplicate-key", now + 1, true)
        .await
        .unwrap();
    store
        .poll_jobs(
            cairn_core::JobKind::Distill,
            8,
            now + cairn_core::JOB_LEASE_SECONDS,
        )
        .await
        .expect("one expired key must not fail the other kind");
    assert_eq!(
        store
            .job_snapshot(now + cairn_core::JOB_LEASE_SECONDS)
            .await
            .unwrap()
            .pending,
        1
    );
    let retried = store
        .poll_jobs(
            cairn_core::JobKind::Embed,
            8,
            now + cairn_core::JOB_LEASE_SECONDS,
        )
        .await
        .unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].key, "duplicate-key");

    store
        .enqueue_job(cairn_core::JobKind::Embed, "retry-key", now, true)
        .await
        .unwrap();
    let running = store
        .poll_jobs(cairn_core::JobKind::Embed, 8, now)
        .await
        .unwrap()
        .into_iter()
        .find(|job| job.key == "retry-key")
        .unwrap();
    store
        .enqueue_job(cairn_core::JobKind::Embed, "retry-key", now + 1, true)
        .await
        .unwrap();
    let blocked = store
        .poll_jobs(cairn_core::JobKind::Embed, 8, now + 1)
        .await
        .unwrap();
    assert!(
        blocked.iter().all(|job| job.key != "retry-key"),
        "a pending successor was claimed during a live lease"
    );
    store
        .settle_job(
            &running.job_id,
            running.claim_id.as_deref().unwrap(),
            now + 2,
            cairn_core::JobEnd::Retry("temporary".into()),
        )
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_jobs
         WHERE job_key = 'retry-key' AND succeeded = false AND claim_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_quarantine_does_not_reset_attempts_or_starve_backfill() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("jobs-owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let now = cairn_core::now_seconds();
    let mut ids = Vec::new();
    for index in 0..9 {
        let memory = store
            .remember_with_kind(
                &owner.token,
                &format!("Useful old note {index}"),
                vec![],
                WorkContext::default(),
                Default::default(),
                cairn_core::MemoryKind::Explicit,
            )
            .await
            .unwrap();
        ids.push(memory.memory_id);
    }
    sqlx::query("DELETE FROM cairn_jobs")
        .execute(&pool)
        .await
        .unwrap();
    store.reconcile_embeddings(512, now).await.unwrap();
    assert_eq!(store.job_snapshot(now).await.unwrap().pending, 9);
    let mut claimed = Vec::new();
    for _ in 0..3 {
        let batch = store
            .poll_jobs(cairn_core::JobKind::Embed, 8, now)
            .await
            .unwrap();
        if batch.is_empty() {
            break;
        }
        for claim in batch {
            claimed.push(claim.key.clone());
            store
                .settle_job(
                    &claim.job_id,
                    claim.claim_id.as_deref().unwrap(),
                    now,
                    cairn_core::JobEnd::Quarantine("embedding batch length mismatch".into()),
                )
                .await
                .unwrap();
        }
    }
    claimed.sort();
    claimed.dedup();
    assert_eq!(claimed.len(), 9);
    assert_eq!(store.job_snapshot(now).await.unwrap().quarantined, 9);
    assert!(
        store
            .poll_jobs(cairn_core::JobKind::Embed, 8, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.job_snapshot(now).await.unwrap().quarantined, 9);
    let first = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM cairn_job_quarantine WHERE job_key = $1",
    )
    .bind(&ids[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first, 1);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_expired_distill_quarantine_is_not_restarted_by_the_idle_sweep() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("sweep-owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = store
        .register_device(
            &owner.token,
            "sweep-device",
            &owner.account.person_id,
            None,
            "Sweep device",
        )
        .await
        .unwrap();
    let now = cairn_core::now_seconds();
    store
        .start_session(&owner.token, "crash-cap", WorkContext::default())
        .await
        .unwrap();
    store
        .sync_push(&device.token, review_note("crash-cap", "cap-event"))
        .await
        .unwrap();
    let key = cairn_core::distill_key(Some(&owner.account.person_id), "crash-cap");
    store
        .enqueue_job(cairn_core::JobKind::Distill, &key, now, true)
        .await
        .unwrap();
    let mut tick = now;
    for attempt in 0..5 {
        let claimed = store
            .poll_jobs(cairn_core::JobKind::Distill, 8, tick)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1, "attempt {attempt}");
        assert_eq!(claimed[0].attempts, attempt);
        tick += cairn_core::JOB_LEASE_SECONDS;
        assert!(
            store
                .poll_jobs(cairn_core::JobKind::Distill, 8, tick)
                .await
                .unwrap()
                .is_empty()
        );
        if attempt < 4 {
            tick += cairn_core::JOB_BACKOFF_SECONDS[attempt as usize];
        }
    }
    assert_eq!(store.job_snapshot(tick).await.unwrap().quarantined, 1);
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes' WHERE session_id = 'crash-cap'",
    )
    .execute(&pool)
    .await
    .unwrap();
    store.sweep_idle_sessions().await;
    let succeeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_jobs WHERE kind = 'distill' AND succeeded = true",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(succeeded, 0);
    assert_eq!(store.job_snapshot(tick).await.unwrap().quarantined, 1);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_empty_session_close_is_a_normal_outcome_and_clears_pending() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("empty-owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = store
        .register_device(
            &owner.token,
            "empty-device",
            &owner.account.person_id,
            None,
            "Empty device",
        )
        .await
        .unwrap();
    store
        .start_session(&owner.token, "empty-close", WorkContext::default())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes' WHERE session_id = 'empty-close'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let error = store
        .end_session(&owner.token, "empty-close")
        .await
        .unwrap_err();
    assert!(cairn_core::is_empty_session(&error.to_string()));
    let snapshot = store.job_snapshot(cairn_core::now_seconds()).await.unwrap();
    assert_eq!(snapshot.quarantined, 0);
    let succeeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_jobs WHERE kind = 'distill' AND succeeded = true",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(succeeded, 1);
    let pending: bool = sqlx::query_scalar(
        "SELECT pending_since IS NOT NULL FROM cairn_sessions WHERE session_id = 'empty-close'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending, "an empty close clears the pending marker");
    let ended: bool = sqlx::query_scalar(
        "SELECT ended_at IS NOT NULL FROM cairn_sessions WHERE session_id = 'empty-close'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!ended, "an empty close leaves the session open");
    store.sweep_idle_sessions().await;
    assert_eq!(
        store
            .job_snapshot(cairn_core::now_seconds())
            .await
            .unwrap()
            .quarantined,
        0
    );
    store
        .sync_push(&device.token, review_note("empty-close", "later-capture"))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes' WHERE session_id = 'empty-close'",
    )
    .execute(&pool)
    .await
    .unwrap();
    store.sweep_idle_sessions().await;
    let summaries: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cairn_memories WHERE session_id = 'empty-close'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(summaries >= 1, "a later capture is still distilled");
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_empty_close_keeps_a_concurrent_captures_pending_marker() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("empty-race@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = store
        .register_device(
            &owner.token,
            "empty-race-device",
            &owner.account.person_id,
            None,
            "Empty race device",
        )
        .await
        .unwrap();
    let now = cairn_core::now_seconds();
    store
        .start_session(&owner.token, "empty-race", WorkContext::default())
        .await
        .unwrap();
    let key = cairn_core::distill_key(Some(&owner.account.person_id), "empty-race");
    store
        .enqueue_job(cairn_core::JobKind::Distill, &key, now, true)
        .await
        .unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Distill, 1, now)
        .await
        .unwrap()
        .remove(0);
    // SHARE mode lets the distill transaction read an empty session and lock the session row,
    // but blocks its settlement write.
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE cairn_jobs IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let worker = store.clone();
    let task = tokio::spawn(async move { worker.execute_claimed_distill(&claim).await });
    let wait = async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM pg_stat_activity
                     WHERE datname = current_database()
                       AND pid <> pg_backend_pid()
                       AND wait_event_type = 'Lock'
                       AND query ILIKE '%cairn_jobs%'
                 )",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), wait)
        .await
        .expect("distill settlement did not wait on the job lock");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let pusher = store.clone();
    let device_token = device.token.clone();
    let push = tokio::spawn(async move {
        pusher
            .sync_push(&device_token, review_note("empty-race", "racing-capture"))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    blocker.commit().await.unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert!(cairn_core::is_empty_session(&error.to_string()));
    push.await.unwrap().unwrap();
    let pending: bool = sqlx::query_scalar(
        "SELECT pending_since IS NOT NULL FROM cairn_sessions WHERE session_id = 'empty-race'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(pending, "the newer capture is still pending distillation");
    assert_eq!(
        store
            .job_snapshot(cairn_core::now_seconds())
            .await
            .unwrap()
            .quarantined,
        0
    );
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes' WHERE session_id = 'empty-race'",
    )
    .execute(&pool)
    .await
    .unwrap();
    store.sweep_idle_sessions().await;
    let summaries: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cairn_memories WHERE session_id = 'empty-race'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(summaries >= 1, "the racing capture is distilled");
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_lost_distill_claim_does_not_commit_side_effects() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("stale-owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = store
        .register_device(
            &owner.token,
            "stale-device",
            &owner.account.person_id,
            None,
            "Stale device",
        )
        .await
        .unwrap();
    let now = 1_700_000_000_u64;
    store
        .start_session(&owner.token, "real-stale", WorkContext::default())
        .await
        .unwrap();
    store
        .sync_push(&device.token, review_note("real-stale", "real-event"))
        .await
        .unwrap();
    let key = cairn_core::distill_key(Some(&owner.account.person_id), "real-stale");
    store
        .enqueue_job(cairn_core::JobKind::Distill, &key, now, true)
        .await
        .unwrap();
    let old = store
        .poll_jobs(cairn_core::JobKind::Distill, 8, now)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .poll_jobs(
                cairn_core::JobKind::Distill,
                8,
                now + cairn_core::JOB_LEASE_SECONDS
            )
            .await
            .unwrap()
            .is_empty()
    );
    let replacement = store
        .poll_jobs(
            cairn_core::JobKind::Distill,
            8,
            now + cairn_core::JOB_LEASE_SECONDS + cairn_core::JOB_BACKOFF_SECONDS[0],
        )
        .await
        .unwrap()
        .remove(0);
    assert_ne!(old.claim_id, replacement.claim_id);
    assert!(store.execute_claimed_distill(&old).await.is_err());
    assert_eq!(
        store
            .job_snapshot(now + cairn_core::JOB_LEASE_SECONDS + cairn_core::JOB_BACKOFF_SECONDS[0])
            .await
            .unwrap()
            .running,
        1
    );
    let summaries: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cairn_memories WHERE session_id = 'real-stale'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(summaries, 0);

    store
        .start_session(&owner.token, "empty-stale", WorkContext::default())
        .await
        .unwrap();
    sqlx::query("UPDATE cairn_sessions SET pending_since = now() WHERE session_id = 'empty-stale'")
        .execute(&pool)
        .await
        .unwrap();
    let fake = cairn_core::BackgroundJob {
        job_id: "nonexistent-job".into(),
        kind: "distill".into(),
        key: cairn_core::distill_key(Some(&owner.account.person_id), "empty-stale"),
        attempts: 0,
        not_before: now,
        claim_id: Some("stale".into()),
        lease_expires_at: Some(now.saturating_sub(1)),
        last_error: None,
        created_at: now,
        updated_at: now,
        finished_at: None,
        succeeded: false,
    };
    assert!(store.execute_claimed_distill(&fake).await.is_err());
    let cleared: bool = sqlx::query_scalar(
        "SELECT pending_since IS NULL FROM cairn_sessions WHERE session_id = 'empty-stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!cleared);
    assert!(
        store
            .job_snapshot(now)
            .await
            .unwrap()
            .quarantine
            .iter()
            .all(|row| row.key != fake.key || row.job_id != fake.job_id)
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_session_id_text_does_not_reclassify_a_distill_failure() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("name-owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let now = cairn_core::now_seconds();
    assert!(
        store
            .end_session(&owner.token, "unknown forgotten or superseded")
            .await
            .is_err()
    );
    let snapshot = store.job_snapshot(now).await.unwrap();
    assert!(
        snapshot
            .quarantine
            .iter()
            .any(|row| row.last_error.contains("unknown session"))
    );
    let succeeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_jobs WHERE kind = 'distill' AND succeeded = true",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(succeeded, 0);
    sqlx::query("TRUNCATE cairn_jobs, cairn_job_quarantine")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .end_session(&owner.token, "database error")
            .await
            .is_err()
    );
    let snapshot = store.job_snapshot(now).await.unwrap();
    assert!(
        snapshot
            .quarantine
            .iter()
            .any(|row| row.last_error.contains("unknown session"))
    );
    assert!(snapshot.pending == 0 || snapshot.quarantine.iter().any(|row| row.attempts == 1));
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cairn_jobs WHERE kind = 'distill' AND succeeded = false",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(waiting, 0);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_enqueue_does_not_rewrite_retained_jobs() {
    let (pool, store) = pool_and_store().await;
    let indexes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_indexes
         WHERE indexname IN ('cairn_jobs_claimable', 'cairn_jobs_running')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(indexes, 2);
    let now = cairn_core::now_seconds();
    for retained in [100_i64, 1_000, 5_000] {
        sqlx::query("TRUNCATE cairn_jobs, cairn_job_quarantine")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO cairn_jobs
                (job_id, kind, job_key, attempts, not_before, created_at, updated_at, succeeded, finished_at)
             SELECT 'bench-' || g, 'embed', 'bench-' || g, 0, $1, $1, $1, true, $1
             FROM generate_series(1, $2) AS g",
        )
        .bind(now as i64)
        .bind(retained)
        .execute(&pool)
        .await
        .unwrap();
        let before: Vec<(String, String)> =
            sqlx::query_as("SELECT ctid::text, job_id FROM cairn_jobs")
                .fetch_all(&pool)
                .await
                .unwrap();
        let started = std::time::Instant::now();
        assert!(
            store
                .enqueue_job(cairn_core::JobKind::Embed, "benchmark-new", now, true)
                .await
                .unwrap()
        );
        let elapsed = started.elapsed();
        let after: Vec<(String, String)> =
            sqlx::query_as("SELECT ctid::text, job_id FROM cairn_jobs")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            before.iter().all(|row| after.contains(row)),
            "enqueue rewrote retained rows at {retained}"
        );
        assert_eq!(after.len(), before.len() + 1);
        eprintln!("enqueue with {retained} retained rows: {elapsed:?}");
    }
}

fn unit_pair(cosine: f32) -> (Vec<f32>, Vec<f32>) {
    let mut left = vec![0.0; 384];
    let mut right = vec![0.0; 384];
    left[0] = 1.0;
    right[0] = cosine;
    right[1] = (1.0 - cosine * cosine).sqrt();
    (left, right)
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_contradiction_shades_reviews_and_supersede_hides() {
    let (_pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let save = |token: &str, scope: &str, title: &str| {
        let store = &store;
        let token = token.to_owned();
        let scope = scope.to_owned();
        let title = title.to_owned();
        async move {
            store
                .save(
                    &token,
                    serde_json::from_value(serde_json::json!({
                        "scope": scope,
                        "kind": "fact",
                        "title": title,
                        "summary": title,
                        "reason": "verified",
                        "statement": title
                    }))
                    .unwrap(),
                )
                .await
                .unwrap()
        }
    };
    let target = save(&owner.token, "personal", "sharedtoken target").await;
    let source = save(&owner.token, "personal", "sharedtoken source").await;
    let before = store
        .search(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .await
        .unwrap();
    let before_target = before
        .iter()
        .find(|hit| hit.memory_id == target.memory_id)
        .unwrap()
        .score;
    let hidden = store
        .declare_contradiction(&other.token, &source.memory_id, &target.memory_id)
        .await
        .unwrap_err();
    assert!(hidden.to_string().contains("unknown memory"));
    store
        .declare_contradiction(&owner.token, &source.memory_id, &target.memory_id)
        .await
        .unwrap();
    let after = store
        .search(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .await
        .unwrap();
    let target_score = after
        .iter()
        .find(|hit| hit.memory_id == target.memory_id)
        .unwrap()
        .score;
    assert!(
        (target_score / before_target - 0.5).abs() < 1e-3,
        "{target_score} {before_target}"
    );
    let global = save(&owner.token, "global", "globaltoken").await;
    let crossed = store
        .declare_contradiction(&owner.token, &source.memory_id, &global.memory_id)
        .await
        .unwrap_err();
    assert!(crossed.to_string().contains("same scope"));
    store
        .supersede_memory(&owner.token, &source.memory_id, &target.memory_id)
        .await
        .unwrap();
    let recalled = store
        .search(&owner.token, "sharedtoken", 5, &RecallFilter::default())
        .await
        .unwrap();
    assert!(recalled.iter().all(|hit| hit.memory_id != target.memory_id));
    let history = store.export_bundle(&owner.token).await.unwrap();
    assert!(
        history
            .memories
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );

    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .await
        .unwrap();
    store
        .join_project(&member.token, &project.project_id)
        .await
        .unwrap();
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    let left = save_project_note(
        &store,
        &owner.token,
        &project.project_id,
        "left-note",
        context.clone(),
    )
    .await;
    let right = save_project_note(
        &store,
        &owner.token,
        &project.project_id,
        "right-note",
        context,
    )
    .await;
    let refused = store
        .declare_contradiction(&member.token, &right.memory_id, &left.memory_id)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("author or project owner"));
    let (low, high) = unit_pair(0.8);
    store
        .put_embedding(cairn_core::MemoryEmbedding::new(&left, low).unwrap())
        .await
        .unwrap();
    store
        .put_embedding(cairn_core::MemoryEmbedding::new(&right, high).unwrap())
        .await
        .unwrap();
    let now = 1_000_u64;
    store.ensure_similarity_job(now).await.unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, now)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(store.finish_similarity_scan(&claim, now).await.unwrap(), 1);
    assert!(
        store
            .poll_jobs(
                cairn_core::JobKind::Similarity,
                1,
                now + cairn_core::SIMILARITY_INTERVAL_SECONDS - 1
            )
            .await
            .unwrap()
            .is_empty()
    );
    let reviews = store.similarity_reviews(&owner.token).await.unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].band, cairn_core::SimilarityBand::NearDuplicate);
    assert!(
        store
            .similarity_reviews(&other.token)
            .await
            .unwrap()
            .is_empty()
    );
    let leaked = store
        .dismiss_review(&other.token, &reviews[0].review_id)
        .await
        .unwrap_err();
    assert!(leaked.to_string().contains("unknown review"));
    let member_refused = store
        .dismiss_review(&member.token, &reviews[0].review_id)
        .await
        .unwrap_err();
    assert!(
        member_refused
            .to_string()
            .contains("author or project owner")
    );
    let contradicted = store
        .contradict_review(&owner.token, &reviews[0].review_id, &left.memory_id)
        .await
        .unwrap();
    assert!(contradicted.contradicts.contains(&left.memory_id));
    assert!(
        store
            .similarity_reviews(&owner.token)
            .await
            .unwrap()
            .is_empty()
    );
}

fn stage7_fact(scope: &str, title: &str, project: Option<&str>) -> cairn_core::SaveMemoryInput {
    serde_json::from_value(serde_json::json!({
        "scope": scope,
        "kind": "fact",
        "title": title,
        "summary": title,
        "reason": "verified",
        "statement": title,
        "context": {"project_id": project}
    }))
    .unwrap()
}

fn unit_embedding(memory: &cairn_core::Memory) -> cairn_core::MemoryEmbedding {
    let mut vector = vec![0.0; 384];
    vector[0] = 1.0;
    cairn_core::MemoryEmbedding::new(memory, vector).unwrap()
}

async fn stage7_accounts(
    store: &PostgresStore,
) -> (
    cairn_core::AuthSessionResult,
    cairn_core::AuthSessionResult,
    cairn_core::AuthSessionResult,
    cairn_core::Project,
) {
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let left = store
        .register_account("left@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let right = store
        .register_account("right@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .await
        .unwrap();
    store
        .join_project(&left.token, &project.project_id)
        .await
        .unwrap();
    store
        .join_project(&right.token, &project.project_id)
        .await
        .unwrap();
    (owner, left, right, project)
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_dismiss_requires_edit_rights_on_both_project_memories() {
    let store = PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let (owner, left_author, right_author, project) = stage7_accounts(&store).await;
    let left = store
        .save(
            &left_author.token,
            stage7_fact("project", "dismiss-left", Some(&project.project_id)),
        )
        .await
        .unwrap();
    let right = store
        .save(
            &right_author.token,
            stage7_fact("project", "dismiss-right", Some(&project.project_id)),
        )
        .await
        .unwrap();
    store.put_embedding(unit_embedding(&left)).await.unwrap();
    store.put_embedding(unit_embedding(&right)).await.unwrap();
    store.ensure_similarity_job(1_000).await.unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, 1_000)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        store.finish_similarity_scan(&claim, 1_000).await.unwrap(),
        1
    );
    let review = store
        .similarity_reviews(&left_author.token)
        .await
        .unwrap()
        .into_iter()
        .find(|review| review.left_id == left.memory_id || review.right_id == left.memory_id)
        .unwrap();
    let declare = store
        .declare_contradiction(&left_author.token, &left.memory_id, &right.memory_id)
        .await
        .unwrap_err();
    assert!(declare.to_string().contains("author or project owner"));
    let dismissed = store
        .dismiss_review(&left_author.token, &review.review_id)
        .await
        .unwrap_err();
    assert!(dismissed.to_string().contains("author or project owner"));
    store
        .dismiss_review(&owner.token, &review.review_id)
        .await
        .unwrap();
    assert!(
        store
            .similarity_reviews(&owner.token)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_cross_author_project_supersede_hides_the_target() {
    let store = PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .await
        .unwrap();
    store
        .join_project(&member.token, &project.project_id)
        .await
        .unwrap();
    let target = store
        .save(
            &owner.token,
            stage7_fact("project", "crosstoken target", Some(&project.project_id)),
        )
        .await
        .unwrap();
    let source = store
        .save(
            &member.token,
            stage7_fact("project", "crosstoken source", Some(&project.project_id)),
        )
        .await
        .unwrap();
    store
        .supersede_memory(&owner.token, &source.memory_id, &target.memory_id)
        .await
        .unwrap();
    assert!(
        !store
            .memories(&owner.token)
            .await
            .unwrap()
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
    let found = store
        .search(&owner.token, "crosstoken", 10, &RecallFilter::default())
        .await
        .unwrap();
    assert!(found.iter().all(|hit| hit.memory_id != target.memory_id));
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_rederived_summary_keeps_an_explicit_supersede() {
    let store = PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = device_for(&store, &owner, "rederive").await;
    let target = store
        .save(
            &owner.token,
            stage7_fact("personal", "summary-target", None),
        )
        .await
        .unwrap();
    store
        .start_session(&owner.token, "reuse", WorkContext::default())
        .await
        .unwrap();
    store
        .sync_push(
            &device,
            ObservationInput {
                event_id: "first".to_owned(),
                session_id: "reuse".to_owned(),
                content: "Decision: use first because tests passed.".to_owned(),
                role: "user".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: WorkContext::default(),
            },
        )
        .await
        .unwrap();
    let summary = store.end_session(&owner.token, "reuse").await.unwrap();
    store
        .supersede_memory(&owner.token, &summary.memory_id, &target.memory_id)
        .await
        .unwrap();
    assert!(
        !store
            .memories(&owner.token)
            .await
            .unwrap()
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
    store
        .sync_push(
            &device,
            ObservationInput {
                event_id: "second".to_owned(),
                session_id: "reuse".to_owned(),
                content: "Decision: use second because tests passed.".to_owned(),
                role: "user".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: WorkContext::default(),
            },
        )
        .await
        .unwrap();
    let derived = store.end_session(&owner.token, "reuse").await.unwrap();
    assert_eq!(
        derived.supersedes.as_deref(),
        Some(target.memory_id.as_str())
    );
    assert!(
        !store
            .memories(&owner.token)
            .await
            .unwrap()
            .iter()
            .any(|memory| memory.memory_id == target.memory_id)
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_distilled_notes_do_not_inherit_declared_contradictions() {
    let store = PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let device = device_for(&store, &owner, "inherit").await;
    let target = store
        .save(&owner.token, stage7_fact("personal", "shade-target", None))
        .await
        .unwrap();
    store
        .start_session(&owner.token, "inherit", WorkContext::default())
        .await
        .unwrap();
    store
        .sync_push(
            &device,
            ObservationInput {
                event_id: "first".to_owned(),
                session_id: "inherit".to_owned(),
                content: "Fact: unrelated first".to_owned(),
                role: "user".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: WorkContext::default(),
            },
        )
        .await
        .unwrap();
    let summary = store.end_session(&owner.token, "inherit").await.unwrap();
    store
        .declare_contradiction(&owner.token, &summary.memory_id, &target.memory_id)
        .await
        .unwrap();
    assert_eq!(
        cairn_core::contradiction_counts(&store.memories(&owner.token).await.unwrap())
            [&target.memory_id],
        1
    );
    store
        .sync_push(
            &device,
            ObservationInput {
                event_id: "second".to_owned(),
                session_id: "inherit".to_owned(),
                content: "Fact: unrelated second".to_owned(),
                role: "user".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: WorkContext::default(),
            },
        )
        .await
        .unwrap();
    store.end_session(&owner.token, "inherit").await.unwrap();
    let memories = store.memories(&owner.token).await.unwrap();
    assert_eq!(
        cairn_core::contradiction_counts(&memories)[&target.memory_id],
        1
    );
    assert!(memories.iter().all(|memory| {
        memory.kind != cairn_core::MemoryKind::Fact
            || !memory.contradicts.contains(&target.memory_id)
    }));
    let successor = store
        .save(
            &owner.token,
            stage7_fact("personal", "summary-successor", None),
        )
        .await
        .unwrap();
    store
        .supersede_memory(&owner.token, &successor.memory_id, &summary.memory_id)
        .await
        .unwrap();
    let remaining = cairn_core::contradiction_counts(&store.memories(&owner.token).await.unwrap());
    assert_eq!(remaining.get(&target.memory_id).copied().unwrap_or(0), 0);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_owner_transfer_waits_for_the_in_flight_edit() {
    let (pool, store) = pool_and_store().await;
    let (owner, left_author, right_author, project) = stage7_accounts(&store).await;
    let left = store
        .save(
            &left_author.token,
            stage7_fact("project", "race-left", Some(&project.project_id)),
        )
        .await
        .unwrap();
    let right = store
        .save(
            &right_author.token,
            stage7_fact("project", "race-right", Some(&project.project_id)),
        )
        .await
        .unwrap();
    let (ordered_left, ordered_right) = cairn_core::ordered_pair(&left.memory_id, &right.memory_id);
    sqlx::query(
        "INSERT INTO cairn_similarity_reviews
            (review_id, left_id, right_id, scope, project_id, cosine, band, status, created_at)
         VALUES ('race-review', $1, $2, 'project', $3, 1, 'near_duplicate', 'open', 1000)",
    )
    .bind(&ordered_left)
    .bind(&ordered_right)
    .bind(&project.project_id)
    .execute(&pool)
    .await
    .unwrap();
    let mut block = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT review_id FROM cairn_similarity_reviews WHERE review_id = 'race-review' FOR UPDATE",
    )
    .fetch_one(&mut *block)
    .await
    .unwrap();
    let editing = store.clone();
    let owner_token = owner.token.clone();
    let left_id = left.memory_id.clone();
    let right_id = right.memory_id.clone();
    let edit = tokio::spawn(async move {
        editing
            .declare_contradiction(&owner_token, &left_id, &right_id)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM pg_stat_activity
                    WHERE datname = current_database()
                      AND wait_event_type = 'Lock'
                      AND query LIKE '%UPDATE cairn_similarity_reviews%'
                )",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let transferring = store.clone();
    let transfer_token = owner.token.clone();
    let project_id = project.project_id.clone();
    let new_owner = left_author.account.person_id.clone();
    let transfer = tokio::spawn(async move {
        transferring
            .transfer_project(&transfer_token, &project_id, &new_owner)
            .await
    });
    let finished_early = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if transfer.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        finished_early.is_err(),
        "owner transfer committed while the edit still held the review row"
    );
    block.commit().await.unwrap();
    let edited = edit.await.unwrap().unwrap();
    assert!(edited.contradicts.contains(&right.memory_id));
    transfer.await.unwrap().unwrap();
    let after = store
        .declare_contradiction(&owner.token, &left.memory_id, &right.memory_id)
        .await;
    assert!(after.is_err());
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_expired_similarity_claim_publishes_nothing() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let left = store
        .save(&owner.token, stage7_fact("global", "scan-a", None))
        .await
        .unwrap();
    let right = store
        .save(&owner.token, stage7_fact("global", "scan-b", None))
        .await
        .unwrap();
    store.put_embedding(unit_embedding(&left)).await.unwrap();
    store.put_embedding(unit_embedding(&right)).await.unwrap();
    store.ensure_similarity_job(1_000).await.unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, 1_000)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .poll_jobs(cairn_core::JobKind::Similarity, 1, 1_121)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.finish_similarity_scan(&claim, 1_121).await.unwrap(),
        0
    );
    assert!(
        store
            .similarity_reviews(&owner.token)
            .await
            .unwrap()
            .is_empty()
    );
    let succeeded: bool = sqlx::query_scalar("SELECT succeeded FROM cairn_jobs WHERE job_id = $1")
        .bind(&claim.job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!succeeded);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_overlapping_scan_does_not_reopen_a_declared_pair() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let mut memories = Vec::new();
    for title in ["scan-left", "scan-right"] {
        let memory = store
            .save(&owner.token, stage7_fact("personal", title, None))
            .await
            .unwrap();
        store.put_embedding(unit_embedding(&memory)).await.unwrap();
        memories.push(memory);
    }
    // The trigger only pauses the insert so a real declaration can commit first.
    sqlx::raw_sql(
        "CREATE FUNCTION pause_review() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(777777); RETURN NEW; END $$;
         CREATE TRIGGER pause_review BEFORE INSERT ON cairn_similarity_reviews
         FOR EACH ROW EXECUTE FUNCTION pause_review();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut block = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(777777)")
        .execute(&mut *block)
        .await
        .unwrap();
    store.ensure_similarity_job(1_000).await.unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, 1_000)
        .await
        .unwrap()
        .remove(0);
    let worker = store.clone();
    let scan = tokio::spawn(async move { worker.finish_similarity_scan(&claim, 1_000).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM pg_stat_activity
                    WHERE datname = current_database()
                      AND wait_event_type = 'Lock'
                      AND wait_event = 'advisory'
                )",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    store
        .declare_contradiction(&owner.token, &memories[1].memory_id, &memories[0].memory_id)
        .await
        .unwrap();
    assert!(
        store
            .similarity_reviews(&owner.token)
            .await
            .unwrap()
            .is_empty()
    );
    block.commit().await.unwrap();
    assert_eq!(scan.await.unwrap().unwrap(), 0);
    assert!(
        store
            .similarity_reviews(&owner.token)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_links_permissions_visibility_cap_and_one_hop() {
    let (pool, store) = pool_and_store().await;
    let migrated: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cairn_schema_migrations WHERE version = '0018_memory_links')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(migrated);
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let seed = store
        .save(&owner.token, stage7_fact("personal", "seedtoken", None))
        .await
        .unwrap();
    let neighbour = store
        .save(&owner.token, stage7_fact("personal", "othertoken", None))
        .await
        .unwrap();
    let alone = store
        .save(&owner.token, stage7_fact("personal", "gammatoken", None))
        .await
        .unwrap();
    let foreign = store
        .save(&member.token, stage7_fact("personal", "foreigntoken", None))
        .await
        .unwrap();
    let hidden = store
        .link_memories(
            &member.token,
            &seed.memory_id,
            cairn_core::LinkRelation::Causes,
            &neighbour.memory_id,
        )
        .await
        .unwrap_err();
    assert!(hidden.to_string().contains("unknown memory"));
    let global = store
        .save(&owner.token, stage7_fact("global", "globaltoken", None))
        .await
        .unwrap();
    let crossed = store
        .link_memories(
            &owner.token,
            &seed.memory_id,
            cairn_core::LinkRelation::Causes,
            &global.memory_id,
        )
        .await
        .unwrap_err();
    assert!(crossed.to_string().contains("same scope"));
    store
        .link_memories(
            &owner.token,
            &seed.memory_id,
            cairn_core::LinkRelation::Causes,
            &neighbour.memory_id,
        )
        .await
        .unwrap();
    store
        .link_memories(
            &owner.token,
            &seed.memory_id,
            cairn_core::LinkRelation::Causes,
            &neighbour.memory_id,
        )
        .await
        .unwrap();
    let stored = store
        .memories(&owner.token)
        .await
        .unwrap()
        .into_iter()
        .find(|memory| memory.memory_id == seed.memory_id)
        .unwrap();
    assert_eq!(stored.links.len(), 1);
    sqlx::query("UPDATE cairn_memories SET links = links || $1::jsonb WHERE memory_id = $2")
        .bind(serde_json::json!([
            { "relation": "fixes", "target_id": foreign.memory_id },
            { "relation": "causes", "target_id": global.memory_id }
        ]))
        .bind(&seed.memory_id)
        .execute(&pool)
        .await
        .unwrap();
    let hits = store
        .search(&owner.token, "seedtoken", 10, &RecallFilter::default())
        .await
        .unwrap();
    let ids = hits
        .iter()
        .map(|hit| hit.memory_id.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&neighbour.memory_id.as_str()));
    assert!(!ids.contains(&alone.memory_id.as_str()));
    assert!(!ids.contains(&foreign.memory_id.as_str()));
    assert!(!ids.contains(&global.memory_id.as_str()));
    let global_hits = store
        .search(&owner.token, "globaltoken", 10, &RecallFilter::default())
        .await
        .unwrap();
    assert!(
        global_hits
            .iter()
            .any(|hit| hit.memory_id == global.memory_id)
    );
    assert!(
        global_hits
            .iter()
            .all(|hit| hit.memory_id != seed.memory_id)
    );
    let graph = store
        .memory_graph(&owner.token, &cairn_core::GraphFilter::default(), None)
        .await
        .unwrap();
    assert!(
        graph
            .nodes
            .iter()
            .all(|node| node.memory_id != foreign.memory_id)
    );
    assert!(graph.edges.iter().all(|edge| {
        edge.source_id != foreign.memory_id && edge.target_id != foreign.memory_id
    }));
    assert!(
        graph.edges.iter().all(|edge| {
            !(edge.source_id == seed.memory_id && edge.target_id == global.memory_id)
        })
    );
    store
        .declare_contradiction(&owner.token, &seed.memory_id, &neighbour.memory_id)
        .await
        .unwrap();
    let drawn = store
        .memory_graph(&owner.token, &cairn_core::GraphFilter::default(), Some(120))
        .await
        .unwrap();
    assert!(
        drawn
            .edges
            .iter()
            .any(|edge| edge.relation == cairn_core::GraphRelation::Contradicts)
    );
    store
        .supersede_memory(&owner.token, &seed.memory_id, &neighbour.memory_id)
        .await
        .unwrap();
    let hidden_graph = store
        .memory_graph(&owner.token, &cairn_core::GraphFilter::default(), Some(500))
        .await
        .unwrap();
    assert!(
        hidden_graph
            .nodes
            .iter()
            .all(|node| node.memory_id != neighbour.memory_id)
    );
    assert!(
        hidden_graph
            .edges
            .iter()
            .all(|edge| edge.target_id != neighbour.memory_id)
    );
    assert!(
        store
            .memory_graph(&owner.token, &cairn_core::GraphFilter::default(), Some(119))
            .await
            .unwrap_err()
            .to_string()
            .contains("120")
    );

    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .await
        .unwrap();
    store
        .join_project(&member.token, &project.project_id)
        .await
        .unwrap();
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    let left = save_project_note(
        &store,
        &owner.token,
        &project.project_id,
        "left-note",
        context.clone(),
    )
    .await;
    let right = save_project_note(
        &store,
        &owner.token,
        &project.project_id,
        "right-note",
        context,
    )
    .await;
    let refused = store
        .link_memories(
            &member.token,
            &left.memory_id,
            cairn_core::LinkRelation::Fixes,
            &right.memory_id,
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("author or project owner"));
    store
        .link_memories(
            &owner.token,
            &left.memory_id,
            cairn_core::LinkRelation::Fixes,
            &right.memory_id,
        )
        .await
        .unwrap();

    let mut hub = String::new();
    for index in 0..121 {
        let title = format!("capnode-{index:03}");
        let memory = store
            .save(&owner.token, stage7_fact("global", &title, None))
            .await
            .unwrap();
        if index == 0 {
            hub = memory.memory_id;
        } else if index < 4 {
            store
                .link_memories(
                    &owner.token,
                    &hub,
                    cairn_core::LinkRelation::Relates,
                    &memory.memory_id,
                )
                .await
                .unwrap();
        }
    }
    let capped = store
        .memory_graph(
            &owner.token,
            &cairn_core::GraphFilter {
                scope: Some(cairn_core::MemoryScope::Global),
                ..cairn_core::GraphFilter::default()
            },
            Some(120),
        )
        .await
        .unwrap();
    assert!(capped.truncated);
    assert_eq!(capped.nodes.len(), 120);
    assert_eq!(capped.node_cap, 120);
    assert!(capped.nodes.iter().any(|node| node.memory_id == hub));

    let mut bundle = store.export_bundle(&owner.token).await.unwrap();
    bundle.sessions.clear();
    bundle.observations.clear();
    bundle.forgotten_ids.clear();
    let mut incoming = alone.clone();
    incoming.memory_id = "imported-cross-scope".to_owned();
    incoming.fields.title = "plain carton".to_owned();
    incoming.fields.summary = "plain carton".to_owned();
    incoming.fields.statement = Some("plain carton".to_owned());
    incoming.content = "plain carton".to_owned();
    incoming.links = vec![
        cairn_core::MemoryLink {
            relation: cairn_core::LinkRelation::Causes,
            target_id: global.memory_id.clone(),
        },
        cairn_core::MemoryLink {
            relation: cairn_core::LinkRelation::Relates,
            target_id: incoming.memory_id.clone(),
        },
        cairn_core::MemoryLink {
            relation: cairn_core::LinkRelation::Fixes,
            target_id: alone.memory_id.clone(),
        },
        cairn_core::MemoryLink {
            relation: cairn_core::LinkRelation::Fixes,
            target_id: alone.memory_id.clone(),
        },
        cairn_core::MemoryLink {
            relation: cairn_core::LinkRelation::DependsOn,
            target_id: "missing-target".to_owned(),
        },
    ];
    bundle.memories = vec![incoming];
    let imported = store.import_bundle(&owner.token, bundle).await.unwrap();
    assert_eq!(imported.imported_memories, 1);
    assert!(imported.dropped_links.iter().any(|link| {
        link.reason == cairn_core::DroppedLinkReason::Scope && link.target_id == global.memory_id
    }));
    assert!(
        imported
            .dropped_links
            .iter()
            .any(|link| link.reason == cairn_core::DroppedLinkReason::SelfLink)
    );
    assert!(
        imported
            .dropped_links
            .iter()
            .any(|link| link.reason == cairn_core::DroppedLinkReason::Duplicate)
    );
    assert!(imported.dropped_links.iter().any(|link| {
        link.reason == cairn_core::DroppedLinkReason::UnknownTarget
            && link.target_id == "missing-target"
    }));
    let stored_import = store
        .memories(&owner.token)
        .await
        .unwrap()
        .into_iter()
        .find(|memory| memory.memory_id == "imported-cross-scope")
        .unwrap();
    assert_eq!(stored_import.links.len(), 1);
    assert_eq!(stored_import.links[0].target_id, alone.memory_id);
    let leaked = store
        .search(&owner.token, "globaltoken", 20, &RecallFilter::default())
        .await
        .unwrap();
    assert!(
        leaked
            .iter()
            .all(|hit| hit.memory_id != "imported-cross-scope")
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_similarity_scan_reads_the_coldest_current_page() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let vector = {
        let mut values = vec![0.0; 384];
        values[0] = 1.0;
        values
    };
    let mut stale = Vec::new();
    let mut current = Vec::new();
    for index in 0..60 {
        let bare = store
            .save(
                &owner.token,
                stage7_fact("personal", &format!("bare-{index}"), None),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE cairn_memories SET access_count = 0 WHERE memory_id = $1")
            .bind(&bare.memory_id)
            .execute(&pool)
            .await
            .unwrap();
        let memory = store
            .save(
                &owner.token,
                stage7_fact("personal", &format!("stale-{index}"), None),
            )
            .await
            .unwrap();
        store
            .put_embedding(cairn_core::MemoryEmbedding::new(&memory, vector.clone()).unwrap())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE cairn_memories SET access_count = 1, record = jsonb_set(record, '{title}', to_jsonb($2::text)) WHERE memory_id = $1",
        )
        .bind(&memory.memory_id)
        .bind(format!("rewritten-{index}"))
        .execute(&pool)
        .await
        .unwrap();
        stale.push(memory.memory_id);
        let memory = store
            .save(
                &owner.token,
                stage7_fact("personal", &format!("cold-{index}"), None),
            )
            .await
            .unwrap();
        store
            .put_embedding(cairn_core::MemoryEmbedding::new(&memory, vector.clone()).unwrap())
            .await
            .unwrap();
        sqlx::query("UPDATE cairn_memories SET access_count = 2 WHERE memory_id = $1")
            .bind(&memory.memory_id)
            .execute(&pool)
            .await
            .unwrap();
        current.push(memory.memory_id);
    }
    let warm = store
        .save(&owner.token, stage7_fact("personal", "warm-extra", None))
        .await
        .unwrap();
    store
        .put_embedding(cairn_core::MemoryEmbedding::new(&warm, vector.clone()).unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE cairn_memories SET access_count = 3 WHERE memory_id = $1")
        .bind(&warm.memory_id)
        .execute(&pool)
        .await
        .unwrap();
    let capture = store
        .save(&owner.token, stage7_fact("personal", "capture-row", None))
        .await
        .unwrap();
    store
        .put_embedding(cairn_core::MemoryEmbedding::new(&capture, vector).unwrap())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE cairn_memories SET kind = 'capture', access_count = 0 WHERE memory_id = $1",
    )
    .bind(&capture.memory_id)
    .execute(&pool)
    .await
    .unwrap();
    let now = 1_000_u64;
    store.ensure_similarity_job(now).await.unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, now)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        store.finish_similarity_scan(&claim, now).await.unwrap(),
        cairn_core::SIMILARITY_REVIEW_CAP
    );
    let reviews = store.similarity_reviews(&owner.token).await.unwrap();
    assert_eq!(reviews.len(), cairn_core::SIMILARITY_REVIEW_CAP);
    let current_ids = current
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
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
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_similarity_cutoff_uses_the_same_byte_order_as_the_file_store() {
    let path = std::env::temp_dir().join(format!(
        "cairn-similarity-cutoff-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut file = cairn_core::MemoryStore::open(&path).unwrap();
    let file_account = file
        .bootstrap_account("review@example.test", "correct horse battery staple")
        .unwrap();
    let template = file
        .save_for_account(
            &file_account.token,
            serde_json::from_value(serde_json::json!({
                "scope": "personal",
                "kind": "fact",
                "title": "template",
                "summary": "boundary probe",
                "reason": "review",
                "statement": "boundary probe"
            }))
            .unwrap(),
        )
        .unwrap();
    let mut bundle = file.export_for_account(&file_account.token).unwrap();
    let mut ids = (0..59)
        .map(|index| format!("0{index:02}"))
        .collect::<Vec<_>>();
    ids.extend(["aZ".to_owned(), "aa".to_owned()]);
    bundle.memories = ids
        .iter()
        .map(|id| {
            let mut memory = template.clone();
            memory.memory_id = id.clone();
            memory.created_at = "1800000000".to_owned();
            memory.access_count = 0;
            memory.source_event_ids.clear();
            memory.fields.title = id.clone();
            memory.fields.summary = id.clone();
            memory
        })
        .collect();
    let store = PostgresStore::connect(
        &std::env::var("CAIRN_TEST_DATABASE_URL").expect("CAIRN_TEST_DATABASE_URL"),
    )
    .await
    .unwrap();
    let account = store
        .bootstrap_account("review@example.test", "correct horse battery staple")
        .await
        .unwrap();
    file.import_for_account(&file_account.token, bundle.clone())
        .unwrap();
    store
        .import_bundle(&account.token, bundle.clone())
        .await
        .unwrap();
    // The template sits outside the coldest page. It has no embedding either way.
    let mut disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for memory in disk["memories"].as_array_mut().unwrap() {
        if memory["memory_id"] == template.memory_id {
            memory["access_count"] = serde_json::json!(100);
        }
    }
    std::fs::write(&path, serde_json::to_vec(&disk).unwrap()).unwrap();
    let mut file = cairn_core::MemoryStore::open(&path).unwrap();
    for (index, memory) in bundle.memories.iter().enumerate() {
        let mut vector = vec![0.0_f32; 384];
        let axis = if memory.memory_id == "aZ" { 0 } else { index };
        vector[axis] = 1.0;
        let embedding = cairn_core::MemoryEmbedding::new(memory, vector).unwrap();
        file.put_embedding(embedding.clone()).unwrap();
        store.put_embedding(embedding).await.unwrap();
    }
    let now = 1_800_000_000_u64;
    file.ensure_similarity_job(now).unwrap();
    let file_claim = file
        .poll_jobs(cairn_core::JobKind::Similarity, 1, now)
        .unwrap()
        .remove(0);
    file.finish_similarity_scan(&file_claim, now).unwrap();
    store.ensure_similarity_job(now).await.unwrap();
    let postgres_claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, now)
        .await
        .unwrap()
        .remove(0);
    store
        .finish_similarity_scan(&postgres_claim, now)
        .await
        .unwrap();
    let file_pairs = file
        .similarity_reviews_for_account(&file_account.token)
        .unwrap()
        .into_iter()
        .map(|review| (review.left_id, review.right_id))
        .collect::<Vec<_>>();
    let postgres_pairs = store
        .similarity_reviews(&account.token)
        .await
        .unwrap()
        .into_iter()
        .map(|review| (review.left_id, review.right_id))
        .collect::<Vec<_>>();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        file_pairs,
        vec![("000".to_owned(), "aZ".to_owned())],
        "byte order keeps aZ inside the coldest 60 and drops aa"
    );
    assert_eq!(
        postgres_pairs, file_pairs,
        "the same ids and vectors must review the same pair"
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_capture_recall_quality_matches_the_file_floor() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url)
        .await
        .expect("connect and migrate PostgreSQL");
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .await
        .expect("bootstrap");
    let project = store
        .register_project(&owner.token, "cairn-demo", "Demo", None)
        .await
        .expect("project");
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "project",
                "kind": "rule",
                "title": "Person-scoped sync events",
                "summary": "Always use cairnfloorrule ownership checks inside one person.",
                "reason": "A global lookup leaked another person's event.",
                "rule": "cairnfloorrule keeps sync_event_id collisions inside the authenticated person.",
                "context": {"project_id": project.project_id}
            }))
            .unwrap(),
        )
        .await
        .expect("rule");
    let device = store
        .register_device(
            &owner.token,
            "quality-device",
            &owner.account.person_id,
            None,
            "test",
        )
        .await
        .expect("device");
    store
        .sync_push(
            &device.token,
            ObservationInput {
                event_id: "evt-raw".to_owned(),
                session_id: "quality".to_owned(),
                content:
                    "Read {\"path\":\"cairnfloorrule.rs\",\"note\":\"do not use cairnfloorrule for ownership checks\"}"
                        .to_owned(),
                role: "tool".to_owned(),
                source: "PostToolUse".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: context.clone(),
            },
        )
        .await
        .expect("capture");
    let hidden = RecallFilter {
        context: Some(context.clone()),
        include_captures: false,
        relevance_floor: true,
        ..RecallFilter::default()
    };
    let hits = store
        .search(&owner.token, "cairnfloorrule", 10, &hidden)
        .await
        .expect("search");
    assert!(
        hits.iter()
            .any(|hit| hit.kind == cairn_core::MemoryKind::Rule)
    );
    assert!(
        hits.iter()
            .all(|hit| hit.kind != cairn_core::MemoryKind::Capture)
    );
    assert!(hits.iter().all(|hit| hit.contradiction_ids.is_empty()));
    let mut opted = hidden.clone();
    opted.include_captures = true;
    let with_capture = store
        .search(&owner.token, "cairnfloorrule", 10, &opted)
        .await
        .expect("opt-in search");
    assert!(
        with_capture
            .iter()
            .any(|hit| hit.kind == cairn_core::MemoryKind::Capture)
    );
    assert!(
        store
            .search(&owner.token, "menu", 10, &hidden)
            .await
            .expect("weak search")
            .is_empty()
    );
    store
        .start_session(&owner.token, "svc-notes", context.clone())
        .await
        .expect("config session");
    store
        .sync_push(
            &device.token,
            ObservationInput {
                event_id: "evt-config".to_owned(),
                session_id: "svc-notes".to_owned(),
                content: "Copied the service config into the notes.".to_owned(),
                role: "assistant".to_owned(),
                source: "user".to_owned(),
                importance: None,
                lifecycle: Default::default(),
                context: context.clone(),
            },
        )
        .await
        .expect("config note");
    let config_summary = store
        .end_session(&owner.token, "svc-notes")
        .await
        .expect("config summary");
    store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "project",
                "kind": "rule",
                "title": "Reload procedure",
                "summary": "config reload waits for the process.",
                "reason": "A short generated summary must not win on one ordinary word.",
                "rule": "config reload uses the published command.",
                "context": {"project_id": project.project_id}
            }))
            .unwrap(),
        )
        .await
        .expect("reload rule");
    let config_hits = store
        .search(&owner.token, "config reload", 10, &hidden)
        .await
        .expect("config search");
    assert!(
        config_hits
            .iter()
            .any(|hit| hit.kind == cairn_core::MemoryKind::Rule)
    );
    assert!(
        config_hits
            .iter()
            .all(|hit| hit.memory_id != config_summary.memory_id)
    );
    let resumed = store
        .resume_context(&owner.token, 10, &hidden)
        .await
        .expect("resume");
    assert!(
        resumed
            .results
            .iter()
            .any(|hit| hit.memory_id == config_summary.memory_id)
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_checkpoint_is_personal_to_the_worktree() {
    let database_url = std::env::var("CAIRN_TEST_DATABASE_URL").expect("CAIRN_TEST_DATABASE_URL");
    let store = PostgresStore::connect(&database_url)
        .await
        .expect("connect");
    let owner = store
        .bootstrap_account("ckpt-owner@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let other = store
        .register_account_as_admin(
            &owner.token,
            "ckpt-other@example.test",
            "correct horse battery staple",
        )
        .await
        .expect("other");
    let (project, _) = store
        .init_project(&owner.token, "local:cairn", Some("cairn"))
        .await
        .expect("project");
    store
        .join_project(&other.token, &project.project_id)
        .await
        .expect("join");
    let agent_a = store
        .register_device(
            &owner.token,
            "ckpt-dev-a",
            &owner.account.person_id,
            Some("agent-a"),
            "A",
        )
        .await
        .expect("device a");
    let agent_b = store
        .register_device(
            &owner.token,
            "ckpt-dev-b",
            &owner.account.person_id,
            Some("agent-b"),
            "B",
        )
        .await
        .expect("device b");
    let work = WorkContext {
        project_id: Some(project.project_id.clone()),
        repository: Some("local:cairn".into()),
        branch: Some("feature".into()),
        worktree: Some("wt-a".into()),
        ..WorkContext::default()
    };
    store
        .start_session(&agent_a.token, "sess-cross", work.clone())
        .await
        .expect("start");
    store
        .sync_push(
            &agent_a.token,
            ObservationInput {
                event_id: "evt-cross".into(),
                session_id: "sess-cross".into(),
                content: "The next step is to keep the handoff. The issue is not yet verified."
                    .into(),
                role: "assistant".into(),
                source: "PostToolUse".into(),
                importance: None,
                lifecycle: Default::default(),
                context: work.clone(),
            },
        )
        .await
        .expect("push");
    store
        .start_session(&agent_a.token, "sess-empty", work.clone())
        .await
        .expect("empty start");
    assert_eq!(
        store
            .checkpoint_session(&agent_a.token, "sess-empty")
            .await
            .expect("empty"),
        CheckpointStatus::Empty
    );
    assert_eq!(
        store
            .checkpoint_session(&agent_a.token, "sess-cross")
            .await
            .expect("stored"),
        CheckpointStatus::Stored
    );
    store
        .sync_push(
            &agent_a.token,
            ObservationInput {
                event_id: "evt-cross-2".into(),
                session_id: "sess-cross".into(),
                content: "The next step is to record the second observation.".into(),
                role: "assistant".into(),
                source: "PostToolUse".into(),
                importance: None,
                lifecycle: Default::default(),
                context: work.clone(),
            },
        )
        .await
        .expect("second push");
    assert_eq!(
        store
            .checkpoint_session(&agent_a.token, "sess-cross")
            .await
            .expect("refresh"),
        CheckpointStatus::Stored
    );
    assert_eq!(
        store
            .checkpoint_session(&agent_a.token, "sess-cross")
            .await
            .expect("same"),
        CheckpointStatus::Unchanged
    );
    let filter = RecallFilter {
        context: Some(work.clone()),
        ..RecallFilter::default()
    };
    let resumed = store
        .resume_context(&agent_b.token, 5, &filter)
        .await
        .expect("resume");
    assert_eq!(resumed.results.len(), 1);
    assert!(
        resumed.results[0]
            .next_steps
            .iter()
            .any(|step| step.contains("second observation"))
    );
    let owner_notes = store.memories(&agent_b.token).await.expect("notes");
    let brief = cairn_core::session_brief(&owner_notes, &work);
    assert!(brief.contains("sess-cross"), "{brief}");
    let mut other_tree = work.clone();
    other_tree.worktree = Some("wt-b".into());
    let other_checkout = cairn_core::session_brief(&owner_notes, &other_tree);
    assert!(!other_checkout.contains("sess-cross"), "{other_checkout}");
    let stranger = store.memories(&other.token).await.expect("stranger");
    let stranger_brief = cairn_core::session_brief(&stranger, &work);
    assert!(!stranger_brief.contains("sess-cross"), "{stranger_brief}");
    assert!(
        store
            .resume_context(&other.token, 5, &filter)
            .await
            .expect("other resume")
            .results
            .is_empty()
    );
    store
        .end_session(&agent_a.token, "sess-cross")
        .await
        .expect("end");
    assert_eq!(
        store
            .checkpoint_session(&agent_a.token, "sess-cross")
            .await
            .expect("closed"),
        CheckpointStatus::Unchanged
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_identical_save_reuses_one_row_and_merges_evidence() {
    let (pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("dedupe@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let other = store
        .register_account("dedupe-other@example.test", "correct horse battery staple")
        .await
        .unwrap();

    let mut first_input = stage7_fact("personal", "dedupe-note", None);
    first_input.fields.evidence = vec!["docs/a".to_owned()];
    let first = store.save(&owner.token, first_input.clone()).await.unwrap();

    let mut retry_input = first_input.clone();
    retry_input.fields.evidence = vec!["docs/b".to_owned()];
    let retry = store.save(&owner.token, retry_input).await.unwrap();
    assert_eq!(retry.memory_id, first.memory_id);
    assert_eq!(retry.fields.evidence, vec!["docs/a", "docs/b"]);
    assert_eq!(retry.source_event_ids, first.source_event_ids);

    let mut no_op_input = first_input.clone();
    no_op_input.fields.evidence = vec!["docs/b".to_owned()];
    let no_op = store.save(&owner.token, no_op_input).await.unwrap();
    assert_eq!(no_op.memory_id, first.memory_id);
    assert_eq!(no_op.changed_at, retry.changed_at);

    let different_text = store
        .save(
            &owner.token,
            serde_json::from_value(serde_json::json!({
                "scope": "personal",
                "kind": "fact",
                "title": "dedupe-note",
                "summary": "different summary",
                "reason": "verified",
                "statement": "dedupe-note"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(different_text.memory_id, first.memory_id);

    let other_owner = store.save(&other.token, first_input).await.unwrap();
    assert_ne!(other_owner.memory_id, first.memory_id);

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM cairn_memories WHERE person_id = $1")
        .bind(&owner.account.person_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "the retry and the no-op reuse the first row");
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_concurrent_identical_saves_keep_one_row_and_all_evidence() {
    let (_pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("race@example.test", "correct horse battery staple")
        .await
        .unwrap();
    let store = std::sync::Arc::new(store);

    let mut left_input = stage7_fact("personal", "race-note", None);
    left_input.fields.evidence = vec!["left".to_owned()];
    let mut right_input = stage7_fact("personal", "race-note", None);
    right_input.fields.evidence = vec!["right".to_owned()];
    let (left, right) = tokio::join!(
        store.save(&owner.token, left_input),
        store.save(&owner.token, right_input)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.memory_id, right.memory_id);

    let rows = store.memories(&owner.token).await.unwrap();
    assert_eq!(rows.len(), 1);
    let mut evidence = rows[0].fields.evidence.clone();
    evidence.sort();
    assert_eq!(evidence, vec!["left", "right"]);
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_possible_conflict_cosine_publishes_no_review() {
    let (_pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("band@example.test", "correct horse battery staple")
        .await
        .unwrap();
    assert_eq!(
        cairn_core::similarity_band(0.5),
        Some(cairn_core::SimilarityBand::PossibleConflict)
    );
    let left = store
        .save(&owner.token, stage7_fact("personal", "band-left", None))
        .await
        .unwrap();
    let right = store
        .save(&owner.token, stage7_fact("personal", "band-right", None))
        .await
        .unwrap();
    let (low, high) = unit_pair(0.5);
    store
        .put_embedding(cairn_core::MemoryEmbedding::new(&left, low).unwrap())
        .await
        .unwrap();
    store
        .put_embedding(cairn_core::MemoryEmbedding::new(&right, high).unwrap())
        .await
        .unwrap();
    store.ensure_similarity_job(1_000).await.unwrap();
    let claim = store
        .poll_jobs(cairn_core::JobKind::Similarity, 1, 1_000)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        store.finish_similarity_scan(&claim, 1_000).await.unwrap(),
        0
    );
    assert!(
        store
            .similarity_reviews(&owner.token)
            .await
            .unwrap()
            .is_empty()
    );
}
