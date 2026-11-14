use cairn_core::{ObservationInput, RecallFilter, WorkContext};
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
    let promoted = store
        .promote_memory(
            &account.token,
            &project.project_id,
            &branch_memory.memory_id,
            "Confirmed by the cache regression test",
        )
        .await
        .expect("promote branch memory");
    assert_eq!(promoted.kind, cairn_core::MemoryKind::Decision);
    assert_eq!(promoted.scope, cairn_core::MemoryScope::Project);
    assert_eq!(
        promoted.project_id.as_deref(),
        Some(project.project_id.as_str())
    );
    assert_eq!(
        promoted.promoted_from.as_deref(),
        Some(branch_memory.memory_id.as_str())
    );
    assert_eq!(promoted.context.branch, None);
    assert_eq!(promoted.verified_commit.as_deref(), Some("abc1234"));
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
            &branch_memory.memory_id,
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
            &branch_memory.memory_id,
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
    let repeated = store
        .promote_memory(
            &account.token,
            &project.project_id,
            &branch_memory.memory_id,
            "Confirmed by the cache regression test",
        )
        .await
        .expect("repeat promotion");
    assert_eq!(repeated.memory_id, promoted.memory_id);
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
    assert!(actions.contains(&"promotion"));
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
            &branch_memory.memory_id,
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
    assert!(report.superseded_memory_ids.contains(&promoted.memory_id));
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
    let private_memory = store
        .remember_with_kind(
            &account.token,
            "private observation stays on the owner account",
            Vec::new(),
            WorkContext::default(),
            Default::default(),
            cairn_core::MemoryKind::Explicit,
        )
        .await
        .expect("remember private observation");
    let repository_memory = store
        .remember_with_kind(
            &account.token,
            "repository cache decision is ready for the project",
            Vec::new(),
            WorkContext {
                repository: Some("cairn".to_owned()),
                ..WorkContext::default()
            },
            Default::default(),
            cairn_core::MemoryKind::Explicit,
        )
        .await
        .expect("remember repository memory");
    let shared_promoted = store
        .promote_memory(
            &account.token,
            &project.project_id,
            &repository_memory.memory_id,
            "Share the promoted repository decision",
        )
        .await
        .expect("promote repository memory");
    assert!(
        store
            .invite_project_member(&account.token, &project.project_id, "owner@example.test",)
            .await
            .is_err()
    );
    assert!(
        store
            .invite_project_member(&account.token, &project.project_id, "missing@example.test",)
            .await
            .is_err()
    );
    let invited = store
        .invite_project_member(&account.token, &project.project_id, "member@example.test")
        .await
        .expect("invite member");
    let repeated_invite = store
        .invite_project_member(&account.token, &project.project_id, "member@example.test")
        .await
        .expect("repeat invite");
    assert_eq!(repeated_invite.person_id, invited.person_id);
    assert!(
        store
            .share_project_memory(
                &account.token,
                &project.project_id,
                &private_memory.memory_id,
                "member@example.test",
            )
            .await
            .is_err()
    );
    store
        .share_project_memory(
            &account.token,
            &project.project_id,
            &shared_promoted.memory_id,
            "member@example.test",
        )
        .await
        .expect("share promoted memory");
    let shared = store
        .shared_project_memories(&member.token, &project.project_id)
        .await
        .expect("member reads shared memory");
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].memory.memory_id, shared_promoted.memory_id);
    let member_search = store
        .search(
            &member.token,
            "private observation",
            10,
            &RecallFilter::default(),
        )
        .await
        .expect("member search");
    assert!(member_search.is_empty());
    let member_context = store
        .context(
            &member.token,
            "private observation",
            10,
            &RecallFilter::default(),
        )
        .await
        .expect("member context");
    assert!(member_context.results.is_empty());
    let member_project_memories = store
        .project_memories(&member.token, &project.project_id)
        .await
        .expect_err("member project listing is owner-only");
    assert!(member_project_memories.to_string().contains("does not own"));
    let member_correct = store
        .correct(
            &member.token,
            &shared_promoted.memory_id,
            "A member must not rewrite the shared decision.",
            Vec::new(),
            WorkContext::default(),
            Default::default(),
        )
        .await
        .expect_err("member correction fails");
    assert!(member_correct.to_string().contains("does not own"));
    let member_forget = store
        .forget(&member.token, &shared_promoted.memory_id)
        .await
        .expect_err("member forget fails");
    assert!(member_forget.to_string().contains("does not own"));
    store
        .revoke_project_share(
            &account.token,
            &project.project_id,
            &shared_promoted.memory_id,
            "member@example.test",
        )
        .await
        .expect("revoke share");
    let after_revoke = store
        .shared_project_memories(&member.token, &project.project_id)
        .await
        .expect("member read after revoke");
    assert!(after_revoke.is_empty());
    let owner_memories = store
        .project_memories(&account.token, &project.project_id)
        .await
        .expect("owner still lists the memory");
    assert!(
        owner_memories
            .iter()
            .any(|memory| memory.memory_id == shared_promoted.memory_id)
    );
    let exported = store
        .export_bundle(&account.token)
        .await
        .expect("owner still exports the memory");
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == shared_promoted.memory_id)
    );
    let share_audit = store
        .audit_events(&account.token, 100)
        .await
        .expect("share audit");
    assert!(
        share_audit
            .iter()
            .any(|event| event.action == "project_memory_shared")
    );
    assert!(
        share_audit
            .iter()
            .any(|event| event.action == "project_memory_share_revoked")
    );

    store
        .share_project_memory(
            &account.token,
            &project.project_id,
            &shared_promoted.memory_id,
            "member@example.test",
        )
        .await
        .expect("share again");
    let successor = store
        .correct(
            &account.token,
            &shared_promoted.memory_id,
            "The successor repository decision replaces the shared one.",
            Vec::new(),
            WorkContext::default(),
            Default::default(),
        )
        .await
        .expect("owner corrects shared memory");
    let omitted = store
        .shared_project_memories(&member.token, &project.project_id)
        .await
        .expect("superseded share is omitted");
    assert!(
        omitted
            .iter()
            .all(|memory| memory.memory.memory_id != shared_promoted.memory_id)
    );
    store
        .share_project_memory(
            &account.token,
            &project.project_id,
            &successor.memory_id,
            "member@example.test",
        )
        .await
        .expect("share successor");
    let successor_read = store
        .shared_project_memories(&member.token, &project.project_id)
        .await
        .expect("member reads successor");
    assert!(
        successor_read
            .iter()
            .any(|memory| memory.memory.memory_id == successor.memory_id)
    );

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
    assert_eq!(summary.scope, cairn_core::MemoryScope::Worktree);
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
    assert_eq!(late_summary.scope, cairn_core::MemoryScope::Worktree);
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
                    scope: cairn_core::MemoryScope::Session,
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
        .expect_err("imported observation stays outside sync");
    let outside = outside.to_string();
    assert!(outside.contains("event already exists outside sync scope"));
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
    for (branch, expected) in [("feat-x", total + 2), ("feat-y", total)] {
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
async fn postgres_branch_and_worktree_registration_validate_the_owned_project() {
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

    // A foreign known project id, a missing project id, for both operations: one validation error.
    let missing = "project-missing".to_owned();
    for (token, id) in [
        (&outsider.token, &project),
        (&outsider.token, &missing),
        (&owner.token, &missing),
    ] {
        let expected = format!("unknown project: {id}");
        let branch_error = store
            .register_branch(token, id, "feat/evil", None, None, None)
            .await
            .expect_err("branch registration needs an owned project");
        assert!(
            matches!(&branch_error, PostgresError::Validation(text) if *text == expected),
            "branch: {branch_error}"
        );
        let worktree_error = store
            .register_worktree(token, id, None, "/wt/evil", None)
            .await
            .expect_err("worktree registration needs an owned project");
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
    assert!(
        store
            .branches(&outsider.token, &project)
            .await
            .expect("outsider read")
            .is_empty()
    );
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

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_project_actions_never_delete_personal_memory() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database");
    let store = PostgresStore::connect(&url).await.expect("connect");
    let password = "correct horse battery staple";
    let owner = store
        .bootstrap_account("never-owner@example.test", password)
        .await
        .expect("owner");
    let member = store
        .register_account("never-member@example.test", password)
        .await
        .expect("member");
    let project = store
        .register_project(&owner.token, "acme/never", "Never", None)
        .await
        .expect("project")
        .project_id;
    store
        .invite_project_member(&owner.token, &project, "never-member@example.test")
        .await
        .expect("invite");
    store
        .create_project_team(&owner.token, &project, "Core")
        .await
        .expect("team");
    let context = WorkContext {
        repository: Some("acme/never".to_owned()),
        ..WorkContext::default()
    };
    let filter = RecallFilter::default();
    let promote_shared = |content: &'static str| {
        let (store, owner, project, context) = (&store, &owner, &project, &context);
        async move {
            let source = store
                .remember_with_kind(
                    &owner.token,
                    content,
                    Vec::new(),
                    context.clone(),
                    cairn_core::LifecycleInput::default(),
                    cairn_core::MemoryKind::Explicit,
                )
                .await
                .expect("remember");
            let copy = store
                .promote_memory(&owner.token, project, &source.memory_id, "why")
                .await
                .expect("promote");
            store
                .share_project_memory(
                    &owner.token,
                    project,
                    &copy.memory_id,
                    "never-member@example.test",
                )
                .await
                .expect("share");
            (source, copy)
        }
    };
    let ids = |hits: Vec<cairn_core::RecallResult>| {
        let mut ids = hits
            .into_iter()
            .map(|hit| hit.memory_id)
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };

    // Forget and retained delete of the copy, by the project and team owner.
    for (word, delete) in [("forgetcopy", false), ("deletecopy", true)] {
        let content: &'static str = Box::leak(format!("{word} personal fact").into_boxed_str());
        let (source, copy) = promote_shared(content).await;
        assert!(copy.source_event_ids.is_empty());
        assert_eq!(
            copy.promoted_from.as_deref(),
            Some(source.memory_id.as_str())
        );
        let hits = store
            .search(&owner.token, word, 10, &filter)
            .await
            .expect("search");
        let origin = hits.iter().find(|hit| hit.memory_id == copy.memory_id);
        assert_eq!(
            origin.and_then(|hit| hit.promoted_from.as_deref()),
            Some(source.memory_id.as_str()),
            "the owner sees the origin"
        );
        if delete {
            store.delete_memory(&owner.token, &copy.memory_id).await
        } else {
            store.forget(&owner.token, &copy.memory_id).await
        }
        .expect("hide copy");
        let hits = store
            .search(&owner.token, word, 10, &filter)
            .await
            .expect("search");
        assert_eq!(ids(hits), vec![source.memory_id.clone()]);
        let bundle = store.export_bundle(&owner.token).await.expect("export");
        assert!(!bundle.forgotten_ids.contains(&source.memory_id));
        assert!(
            source
                .source_event_ids
                .iter()
                .all(|event| !bundle.forgotten_ids.contains(event))
        );
        assert!(
            store
                .project_memories(&owner.token, &project)
                .await
                .expect("list")
                .is_empty()
        );
        assert!(
            store
                .shared_project_memories(&member.token, &project)
                .await
                .expect("shared")
                .is_empty()
        );
    }

    // Forgetting the personal memory leaves the shared copy; the origin is never exposed.
    let (source, copy) = promote_shared("keepcopy personal fact").await;
    let shared = store
        .shared_project_memories(&member.token, &project)
        .await
        .expect("shared");
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].memory.promoted_from, None);
    assert!(shared[0].memory.source_event_ids.is_empty());
    store
        .forget(&owner.token, &source.memory_id)
        .await
        .expect("forget personal");
    let hits = store
        .search(&owner.token, "keepcopy", 10, &filter)
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory_id, copy.memory_id);
    assert_eq!(hits[0].promoted_from, None);
    let listed = store
        .project_memories(&owner.token, &project)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].promoted_from, None);
    let shared = store
        .shared_project_memories(&member.token, &project)
        .await
        .expect("shared");
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].memory.memory_id, copy.memory_id);
    assert_eq!(shared[0].memory.promoted_from, None);

    // A correction of a personal memory hides its promoted copy; promoting the correction
    // links the new copy to the old copy, never to the personal id.
    let (source, copy) = promote_shared("stalecopy old fact").await;
    let correction = store
        .correct(
            &owner.token,
            &source.memory_id,
            "stalecopy new fact",
            Vec::new(),
            context.clone(),
            cairn_core::LifecycleInput::default(),
        )
        .await
        .expect("correct");
    assert!(
        store
            .project_memories(&owner.token, &project)
            .await
            .expect("list")
            .iter()
            .all(|memory| memory.memory_id != copy.memory_id)
    );
    assert!(
        store
            .shared_project_memories(&member.token, &project)
            .await
            .expect("shared")
            .iter()
            .all(|shared| shared.memory.memory_id != copy.memory_id)
    );
    let hits = store
        .search(&owner.token, "stalecopy", 10, &filter)
        .await
        .expect("search");
    assert_eq!(ids(hits), vec![correction.memory_id.clone()]);
    let next = store
        .promote_memory(&owner.token, &project, &correction.memory_id, "why")
        .await
        .expect("promote correction");
    assert_eq!(next.supersedes.as_deref(), Some(copy.memory_id.as_str()));
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
async fn postgres_capture_registers_project_and_branch_without_a_second_project_for_members() {
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
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].repository, "auto-pg");
    let branches = store
        .branches(&solo.token, &projects[0].project_id)
        .await
        .expect("branches");
    assert_eq!(
        branches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        vec!["feat/x"]
    );

    let shared = store
        .register_project(&owner.token, "shared-pg", "Shared", None)
        .await
        .expect("shared");
    store
        .invite_project_member(&owner.token, &shared.project_id, "reg-m@example.test")
        .await
        .expect("invite");
    let member_dev = device_for(&store, &member, "reg-member").await;
    push_in(
        &store,
        &member_dev,
        "rm",
        "m1",
        ctx("shared-pg", Some("feat/m")),
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
        0
    );
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_repromoting_a_forgotten_copy_creates_a_live_copy() {
    let (_pool, store) = pool_and_store().await;
    let owner = store
        .bootstrap_account("repromote@example.test", "correct horse battery staple")
        .await
        .expect("owner");
    let project = store
        .register_project(&owner.token, "repromote-pg", "R", None)
        .await
        .expect("project");
    let ctx = WorkContext {
        repository: Some("repromote-pg".to_owned()),
        ..WorkContext::default()
    };
    let source = store
        .remember_with_kind(
            &owner.token,
            "repromote personal fact",
            Vec::new(),
            ctx,
            Default::default(),
            cairn_core::MemoryKind::Explicit,
        )
        .await
        .expect("source");
    let first = store
        .promote_memory(&owner.token, &project.project_id, &source.memory_id, "why")
        .await
        .expect("first");
    store
        .forget(&owner.token, &first.memory_id)
        .await
        .expect("forget copy");
    let second = store
        .promote_memory(
            &owner.token,
            &project.project_id,
            &source.memory_id,
            "again",
        )
        .await
        .expect("second");
    assert_ne!(second.memory_id, first.memory_id);
    let listed = store
        .project_memories(&owner.token, &project.project_id)
        .await
        .expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].memory_id, second.memory_id);
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
async fn postgres_idle_sweep_reads_only_pending_sessions_and_backs_off_failures() {
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

    // A permanently failing session (its only observation was forgotten) is retried only
    // after another idle period, not on every push.
    push_in(&store, &dev, "bad", "bad-1", WorkContext::default()).await;
    store.forget(&account.token, "bad-1").await.expect("forget");
    sqlx::query("UPDATE cairn_sessions SET pending_since = now() - interval '31 minutes' WHERE person_id = $1 AND session_id = 'bad'")
        .bind(&person)
        .execute(&pool)
        .await
        .expect("age pending");
    assert_eq!(pending("bad").await, Some(true));
    push_in(&store, &dev, "trigger", "sweep-1", WorkContext::default()).await;
    assert_eq!(pending("bad").await, Some(false), "backed off, not retried");
    assert!(!session_ended(&pool, &person, "bad").await);
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
    assert!(foreign.is_unauthorized() || foreign.to_string().contains("unknown project"));
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

    // A solo project merges with a device token; a shared project does not.
    let mut merges = Vec::new();
    for name in ["feat/solo", "feat/shared"] {
        let branch = store
            .register_branch(&device, &project, name, None, None, None)
            .await
            .expect("branch");
        let memory = store
            .remember_with_kind(
                &device,
                &format!("decision on {name}"),
                Vec::new(),
                WorkContext {
                    repository: Some("acme/devices".to_owned()),
                    branch: Some(name.to_owned()),
                    ..WorkContext::default()
                },
                Default::default(),
                cairn_core::MemoryKind::Explicit,
            )
            .await
            .expect("memory");
        merges.push((branch.branch_id, memory.memory_id));
    }
    store
        .confirm_branch_merge(&device, &project, &merges[0].0, &merges[0].1, "c1", "solo")
        .await
        .expect("solo merge with a device token");
    store
        .invite_project_member(&device, &project, "dev-other@example.test")
        .await
        .expect("device invites a member");
    let refused = store
        .confirm_branch_merge(
            &device,
            &project,
            &merges[1].0,
            &merges[1].1,
            "c2",
            "shared",
        )
        .await
        .expect_err("a shared project needs the web console");
    assert!(refused.to_string().contains(cairn_core::WEB_ONLY_ERROR));
    store
        .confirm_branch_merge(
            &owner.token,
            &project,
            &merges[1].0,
            &merges[1].1,
            "c2",
            "shared",
        )
        .await
        .expect("session merges a shared project");

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
async fn postgres_member_removal_is_web_only_and_merge_waits_for_a_concurrent_invite() {
    let (pool, store) = pool_and_store().await;
    let password = "correct horse battery staple";
    let owner = store
        .bootstrap_account("gate-owner@example.test", password)
        .await
        .expect("owner");
    let other = store
        .register_account("gate-other@example.test", password)
        .await
        .expect("other");
    let device = device_for(&store, &owner, "gate-device").await;
    let project = store
        .register_project(&device, "acme/gate", "Gate", None)
        .await
        .expect("project")
        .project_id;
    let mut branches = Vec::new();
    for name in ["feat/gate", "feat/race"] {
        let branch = store
            .register_branch(&device, &project, name, None, None, None)
            .await
            .expect("branch");
        let memory = store
            .remember_with_kind(
                &device,
                &format!("decision on {name}"),
                Vec::new(),
                WorkContext {
                    repository: Some("acme/gate".to_owned()),
                    branch: Some(name.to_owned()),
                    ..WorkContext::default()
                },
                Default::default(),
                cairn_core::MemoryKind::Explicit,
            )
            .await
            .expect("memory");
        branches.push((branch.branch_id, memory.memory_id));
    }
    store
        .invite_project_member(&device, &project, "gate-other@example.test")
        .await
        .expect("device invites");
    store
        .create_project_team(&device, &project, "Core")
        .await
        .expect("device creates the team");
    store
        .add_project_team_member(&device, &project, "gate-other@example.test")
        .await
        .expect("device adds a team member");

    // A device token cannot empty the project to reopen the solo-merge path.
    let removal = store
        .remove_project_team_member(&device, &project, "gate-other@example.test")
        .await
        .expect_err("removing a member is web-only");
    assert!(removal.to_string().contains(cairn_core::WEB_ONLY_ERROR));
    let merge = store
        .confirm_branch_merge(
            &device,
            &project,
            &branches[0].0,
            &branches[0].1,
            "c1",
            "gate",
        )
        .await
        .expect_err("members still need the web console");
    assert!(merge.to_string().contains(cairn_core::WEB_ONLY_ERROR));
    store
        .remove_project_team_member(&owner.token, &project, "gate-other@example.test")
        .await
        .expect("the web session removes the member");
    store
        .confirm_branch_merge(
            &device,
            &project,
            &branches[0].0,
            &branches[0].1,
            "c1",
            "gate",
        )
        .await
        .expect("a solo project merges with the device again");

    // An invite that is still uncommitted holds a key-share lock on the project row. The
    // device merge must wait for it, then see the member and refuse.
    let mut held = pool.begin().await.expect("begin");
    sqlx::query(
        "INSERT INTO cairn_project_members (owner_person_id, project_id, member_person_id)
         VALUES ($1, $2, $3)",
    )
    .bind(&owner.account.person_id)
    .bind(&project)
    .bind(&other.account.person_id)
    .execute(&mut *held)
    .await
    .expect("uncommitted invite");
    let merger = tokio::spawn({
        let (store, device, project) = (store.clone(), device.clone(), project.clone());
        let (branch_id, memory_id) = branches[1].clone();
        async move {
            store
                .confirm_branch_merge(&device, &project, &branch_id, &memory_id, "c2", "race")
                .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !merger.is_finished(),
        "the merge must wait for the in-flight invite"
    );
    held.commit().await.expect("commit");
    let raced = merger
        .await
        .expect("merge finishes")
        .expect_err("the committed member closes the device merge path");
    assert!(raced.to_string().contains(cairn_core::WEB_ONLY_ERROR));
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
