use cairn_core::{ObservationInput, RecallFilter, WorkContext};
use cairn_storage::PostgresStore;

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
    assert_eq!(store.list_devices(&second.token).await.unwrap().len(), 2);

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
        .sync_push(&resumed.token, input)
        .await
        .expect("replay observation");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.cursor, pushed.cursor);

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
        .rotate_device(&resumed.token, "device-a")
        .await
        .expect("rotate device token");
    assert!(store.sync_pull(&resumed.token, 0).await.is_err());
    assert!(store.sync_pull(&rotated.token, 0).await.is_ok());

    store
        .revoke_device(&second.token, "device-a")
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
}
