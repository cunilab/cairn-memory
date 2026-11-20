use crate::legacy::capture::{Observation, ObservationInput, normalize_observation};
use crate::legacy::lifecycle::{Lifecycle, LifecycleInput, MemoryKind, MemoryScope, Sensitivity};
use crate::legacy::memories::{Memory, build_explicit_memory_record};
use crate::legacy::privacy::REDACTED;
use crate::legacy::sessions::{Session, derive_session_memory};
use crate::legacy::store::MemoryStore;
use crate::legacy::test_support::{
    assert_metadata_redacted, context, metadata_secret_context, observation,
    observation_with_sensitivity, temp_store_path,
};
use crate::legacy::transfer::{EXPORT_FORMAT, EXPORT_VERSION, ExportBundle};
use crate::legacy::types::{StoreError, WorkContext};
use std::fs;

#[test]
fn redacts_sensitive_values_before_persistence() -> Result<(), StoreError> {
    let path = temp_store_path("redaction");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-secret-001", context())?;
    let result = store.capture_observation(observation(
        "evt-sensitive-001",
        "Authorization: Bearer real-test-token api_key=another-test-key",
    ))?;

    assert!(!result.observation.content.contains("real-test-token"));
    assert!(!result.observation.content.contains("another-test-key"));
    assert_eq!(result.observation.content.matches(REDACTED).count(), 2);

    let persisted = fs::read_to_string(&path)?;
    assert!(!persisted.contains("real-test-token"));
    assert!(!persisted.contains("another-test-key"));

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn redacts_private_key_and_mixed_case_sk_and_keeps_sensitivity_floor() -> Result<(), StoreError> {
    const BODY: &str = "c3ludGhldGljLWtleS1ib2R5";
    const SK: &str = "SK-synthetic-example";
    struct Case {
        content: &'static str,
        supplied: Option<Sensitivity>,
        sensitivity: Sensitivity,
        absent: &'static [&'static str],
        present: &'static [&'static str],
    }
    let cases = [
        Case {
            content: "prefix SK-synthetic-example suffix",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Secret,
            absent: &[SK],
            present: &["prefix ", " suffix", REDACTED],
        },
        Case {
            content: "sk-synthetic-example",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &["sk-synthetic-example"],
            present: &[REDACTED],
        },
        Case {
            content: "Sk-synthetic-example",
            supplied: Some(Sensitivity::Sensitive),
            sensitivity: Sensitivity::Secret,
            absent: &["Sk-synthetic-example"],
            present: &[REDACTED],
        },
        Case {
            content: "disk-full, risk-based task-queue",
            supplied: None,
            sensitivity: Sensitivity::Private,
            absent: &[REDACTED],
            present: &["disk-full, risk-based task-queue"],
        },
        Case {
            content: "key=(sk-synthetic-example) and \"SK-synthetic-example\"",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &["sk-synthetic-example", "SK-synthetic-example"],
            present: &["key=(", " and \"", REDACTED],
        },
        Case {
            content: "before\r\n-----BEGIN RSA PRIVATE KEY-----\r\nc3ludGhldGljLWtleS1ib2R5\r\n-----END RSA PRIVATE KEY-----\r\nafter",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "BEGIN RSA PRIVATE KEY"],
            present: &["before", "after", REDACTED],
        },
        Case {
            content: "-----Begin Ec Private Key-----\nc3ludGhldGljLWtleS1ib2R5\n-----End Ec Private Key-----\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "Ec Private Key"],
            present: &[REDACTED],
        },
        Case {
            content: "notes before\n-----BEGIN OPENSSH PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\ntail note stays\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "BEGIN OPENSSH PRIVATE KEY"],
            present: &["notes before", "tail note stays"],
        },
        Case {
            content: "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,SYNTHETICIVVALUE\nc3ludGhldGljLWtleS1ib2R5\nordinary sentence stays\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "SYNTHETICIVVALUE", "Proc-Type"],
            present: &["ordinary sentence stays"],
        },
        Case {
            content: "-----BEGIN ENCRYPTED PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END ENCRYPTED PRIVATE KEY-----\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "ENCRYPTED PRIVATE KEY"],
            present: &[REDACTED],
        },
        Case {
            content: "-----BEGIN PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END PRIVATE KEY-----\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "BEGIN PRIVATE KEY"],
            present: &[REDACTED],
        },
        Case {
            content: "lead -----BEGIN EC PRIVATE KEY----- c3ludGhldGljLWtleS1ib2R5",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "BEGIN EC PRIVATE KEY"],
            present: &["lead ", REDACTED],
        },
        Case {
            content: "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY\nnext prose stays\n",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "BEGIN RSA PRIVATE KEY", "END RSA PRIVATE KEY"],
            present: &["next prose stays", REDACTED],
        },
        Case {
            content: "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY then the note stays\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "END RSA PRIVATE KEY"],
            present: &["then the note stays", REDACTED],
        },
        Case {
            content: "-----Begin Rsa Private Key-----\r\nc3ludGhldGljLWtleS1ib2R5-----End Rsa Private Key\r\nafter glue\r\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY, "End Rsa Private Key"],
            present: &["after glue", REDACTED],
        },
        Case {
            content: "-----BEGIN EC PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END EC PRIVATE KEY-----\nkept complete\n",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[BODY],
            present: &["kept complete", REDACTED],
        },
        Case {
            content: "-----BEGIN RSA PRIVATE KEY-----\nThe armor label is PRIVATE KEY in prose.\n",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Secret,
            absent: &[BODY],
            present: &["The armor label is PRIVATE KEY in prose.", REDACTED],
        },
        Case {
            content: "-----BEGIN RSA PRIVATE KEY\nc3ludGhldGljLWtleS1ib2R5\n",
            supplied: None,
            sensitivity: Sensitivity::Sensitive,
            absent: &[REDACTED],
            present: &[BODY, "-----BEGIN RSA PRIVATE KEY"],
        },
        Case {
            content: "-----BEGIN DSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END DSA PRIVATE KEY-----\n",
            supplied: None,
            sensitivity: Sensitivity::Sensitive,
            absent: &[REDACTED],
            present: &[BODY, "BEGIN DSA PRIVATE KEY"],
        },
        Case {
            content: "-----BEGIN CERTIFICATE-----\nbm90LWEtY2VydA==\n-----END CERTIFICATE-----\n",
            supplied: None,
            sensitivity: Sensitivity::Private,
            absent: &[REDACTED],
            present: &[
                "-----BEGIN CERTIFICATE-----\nbm90LWEtY2VydA==\n-----END CERTIFICATE-----\n",
            ],
        },
        Case {
            content: "-----BEGIN PUBLIC KEY-----\nbm90LWEtcHVibGljLWtleQ==\n-----END PUBLIC KEY-----\n",
            supplied: None,
            sensitivity: Sensitivity::Private,
            absent: &[REDACTED],
            present: &[
                "-----BEGIN PUBLIC KEY-----\nbm90LWEtcHVibGljLWtleQ==\n-----END PUBLIC KEY-----\n",
            ],
        },
        Case {
            content: "-----BEGIN CERTIFICATE-----\nstill visible tail\n",
            supplied: None,
            sensitivity: Sensitivity::Private,
            absent: &[REDACTED],
            present: &["-----BEGIN CERTIFICATE-----", "still visible tail"],
        },
        Case {
            content: "The armor label is PRIVATE KEY in prose.",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Sensitive,
            absent: &[REDACTED],
            present: &["The armor label is PRIVATE KEY in prose."],
        },
        Case {
            content: "caf\u{e9} menu SK-synthetic-example \u{43a}\u{43e}\u{43d}\u{435}\u{446}",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[SK],
            present: &[
                "caf\u{e9} menu ",
                " \u{43a}\u{43e}\u{43d}\u{435}\u{446}",
                REDACTED,
            ],
        },
        Case {
            content: "line\r\nSK-synthetic-example\r\nnext",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[SK],
            present: &["line", "next"],
        },
        Case {
            content: "Authorization: Bearer synthetic-bearer-value api_key=synthetic-api-value token=synthetic-token-value password=synthetic-pass-value",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &[
                "synthetic-bearer-value",
                "synthetic-api-value",
                "synthetic-token-value",
                "synthetic-pass-value",
            ],
            present: &[
                "Authorization: Bearer [REDACTED]",
                "api_key=[REDACTED]",
                "token=[REDACTED]",
                "password=[REDACTED]",
            ],
        },
        Case {
            content: "Password=synthetic-pass-value",
            supplied: None,
            sensitivity: Sensitivity::Secret,
            absent: &["synthetic-pass-value"],
            present: &["Password=[REDACTED]"],
        },
        Case {
            content: "The design review is on Thursday.",
            supplied: Some(Sensitivity::Secret),
            sensitivity: Sensitivity::Secret,
            absent: &[REDACTED],
            present: &["The design review is on Thursday."],
        },
        Case {
            content: "The design review is on Thursday.",
            supplied: Some(Sensitivity::Sensitive),
            sensitivity: Sensitivity::Sensitive,
            absent: &[REDACTED],
            present: &["The design review is on Thursday."],
        },
        Case {
            content: "The design review is on Thursday.",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Private,
            absent: &[REDACTED],
            present: &["The design review is on Thursday."],
        },
        Case {
            content: "confidential roadmap note",
            supplied: Some(Sensitivity::Secret),
            sensitivity: Sensitivity::Secret,
            absent: &[REDACTED],
            present: &["confidential roadmap note"],
        },
        Case {
            content: "confidential roadmap note",
            supplied: Some(Sensitivity::Public),
            sensitivity: Sensitivity::Sensitive,
            absent: &[REDACTED],
            present: &["confidential roadmap note"],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let mut input = observation(&format!("evt-redact-{index}"), case.content);
        input.lifecycle.sensitivity = case.supplied.clone();
        let observation = normalize_observation(&input)?;
        let memory = build_explicit_memory_record(
            case.content,
            Vec::new(),
            WorkContext::default(),
            LifecycleInput {
                sensitivity: case.supplied.clone(),
                ..LifecycleInput::default()
            },
            MemoryKind::Explicit,
        )?;
        for absent in case.absent {
            assert!(
                !observation.content.contains(absent),
                "case {index} observation still has {absent}: {}",
                observation.content
            );
            assert!(
                !memory.content.contains(absent),
                "case {index} memory still has {absent}: {}",
                memory.content
            );
        }
        for present in case.present {
            assert!(
                observation.content.contains(present),
                "case {index} observation missing {present}: {}",
                observation.content
            );
            assert!(
                memory.content.contains(present),
                "case {index} memory missing {present}: {}",
                memory.content
            );
        }
        assert_eq!(
            observation.lifecycle.sensitivity, case.sensitivity,
            "case {index} observation sensitivity"
        );
        assert_eq!(
            memory.lifecycle.sensitivity, case.sensitivity,
            "case {index} memory sensitivity"
        );
    }

    let mut certificates = "-----BEGIN CERTIFICATE-----\n".repeat(2048);
    certificates.push_str("caf\u{e9} stays");
    let kept = normalize_observation(&observation("evt-redact-certs", &certificates))?;
    assert!(kept.content.contains("-----BEGIN CERTIFICATE-----"));
    assert!(kept.content.contains("caf\u{e9} stays"));
    assert!(!kept.content.contains(REDACTED));
    assert_eq!(kept.lifecycle.sensitivity, Sensitivity::Private);

    let path = temp_store_path("redaction-floor");
    let mut store = MemoryStore::open(&path)?;
    let captured = store.capture_observation(observation_with_sensitivity(
        "evt-redact-stored",
        "caf\u{e9} SK-synthetic-example\n-----BEGIN OPENSSH PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n\u{43a}\u{43e}\u{43d}\u{435}\u{446}",
        Some(Sensitivity::Public),
    ))?;
    let remembered = store.remember_with_lifecycle(
        "caf\u{e9} SK-synthetic-example\n-----BEGIN PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END PRIVATE KEY-----\n",
        vec!["SK-synthetic-example".to_owned()],
        context(),
        LifecycleInput {
            sensitivity: Some(Sensitivity::Public),
            ..LifecycleInput::default()
        },
    )?;
    let glued = store.capture_observation(observation_with_sensitivity(
        "evt-redact-glued-footer",
        "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY then the note stays\n",
        Some(Sensitivity::Public),
    ))?;
    assert!(
        !glued.observation.content.contains(BODY),
        "glued footer stored {}",
        glued.observation.content
    );
    assert!(glued.observation.content.contains("then the note stays"));
    assert!(!glued.observation.content.contains("END RSA PRIVATE KEY"));
    assert_eq!(glued.observation.lifecycle.sensitivity, Sensitivity::Secret);
    let glued_json = serde_json::to_string(&glued.observation).expect("glued json");
    assert!(!glued_json.contains(BODY));

    let stricter = store.remember_with_lifecycle(
        "The design review is on Thursday.",
        Vec::new(),
        context(),
        LifecycleInput {
            sensitivity: Some(Sensitivity::Secret),
            ..LifecycleInput::default()
        },
    )?;
    let captured_json = serde_json::to_string(&captured.observation).expect("observation json");
    let remembered_json = serde_json::to_string(&remembered).expect("memory json");
    let persisted = fs::read_to_string(&path)?;
    for secret in [SK, BODY] {
        assert!(!captured.observation.content.contains(secret));
        assert!(!remembered.content.contains(secret));
        assert!(!captured_json.contains(secret));
        assert!(!remembered_json.contains(secret));
        assert!(!persisted.contains(secret));
    }
    assert!(captured.observation.content.contains("caf\u{e9} "));
    assert!(
        captured
            .observation
            .content
            .contains("\u{43a}\u{43e}\u{43d}\u{435}\u{446}")
    );
    assert_eq!(
        captured.observation.lifecycle.sensitivity,
        Sensitivity::Secret
    );
    assert_eq!(remembered.lifecycle.sensitivity, Sensitivity::Secret);
    assert_eq!(stricter.lifecycle.sensitivity, Sensitivity::Secret);
    assert_eq!(stricter.content, "The design review is on Thursday.");
    assert!(persisted.contains("The design review is on Thursday."));
    assert!(persisted.contains("then the note stays"));
    assert!(!persisted.contains("END RSA PRIVATE KEY"));

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn redacts_human_metadata_before_new_persistence() -> Result<(), StoreError> {
    const SK: &str = "SK-synthetic-example";
    const BODY: &str = "c3ludGhldGljLWtleS1ib2R5";
    let secret_context = metadata_secret_context();
    let mut captured_input = observation_with_sensitivity(
        "evt-meta-capture",
        "Ordinary capture note. next step: check the café log",
        Some(Sensitivity::Public),
    );
    captured_input.session_id = "sess-meta".to_owned();
    captured_input.context = secret_context.clone();
    let normalized = normalize_observation(&captured_input)?;
    assert_metadata_redacted(&normalized.context);
    assert_eq!(normalized.event_id, "evt-meta-capture");
    assert_eq!(normalized.lifecycle.sensitivity, Sensitivity::Secret);
    assert!(!normalized.content.contains(SK));
    assert!(normalized.content.contains("café"));

    let remembered = build_explicit_memory_record(
        "Ordinary explicit note.",
        vec![SK.to_owned()],
        secret_context.clone(),
        LifecycleInput {
            sensitivity: Some(Sensitivity::Public),
            ..LifecycleInput::default()
        },
        MemoryKind::Explicit,
    )?;
    assert_metadata_redacted(&remembered.context);
    assert_eq!(remembered.lifecycle.sensitivity, Sensitivity::Secret);
    assert!(!remembered.content.contains(SK));
    assert!(remembered.content.contains("Concepts: [REDACTED]"));
    let stricter = build_explicit_memory_record(
        "Ordinary explicit note.",
        Vec::new(),
        context(),
        LifecycleInput {
            sensitivity: Some(Sensitivity::Secret),
            ..LifecycleInput::default()
        },
        MemoryKind::Explicit,
    )?;
    assert_eq!(stricter.lifecycle.sensitivity, Sensitivity::Secret);
    assert_eq!(stricter.context.commands, context().commands);

    let raw_observation = Observation {
        event_id: "evt-derive-old".to_owned(),
        session_id: "sess-derive".to_owned(),
        content: "ordinary derive source".to_owned(),
        role: "assistant".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: Lifecycle::default(),
        scope: MemoryScope::Personal,
        context: secret_context.clone(),
        captured_at: "1".to_owned(),
    };
    let derived = derive_session_memory(
        None,
        "sess-derive",
        &WorkContext::default(),
        &[raw_observation],
    )?;
    assert_eq!(
        derived.context.commands,
        vec!["echo café [REDACTED]".to_owned()]
    );
    assert_eq!(derived.context.branch.as_deref(), Some("sk-looking-name"));
    assert!(derived.context.handoffs.is_empty());
    assert_eq!(derived.source_event_ids, vec!["evt-derive-old".to_owned()]);
    assert_eq!(derived.lifecycle.sensitivity, Sensitivity::Private);
    assert!(!derived.content.contains(SK));
    assert!(!derived.content.contains(BODY));

    let path = temp_store_path("metadata-redaction");
    let mut store = MemoryStore::open(&path)?;
    let started = store.start_session("sess-meta", secret_context.clone())?;
    assert_metadata_redacted(&started.context);
    let reopened_start = store.start_session(
        "sess-meta",
        WorkContext {
            commands: vec![SK.to_owned()],
            ..WorkContext::default()
        },
    )?;
    assert_eq!(reopened_start.context, started.context);
    assert_eq!(reopened_start.session_id, "sess-meta");
    let inherited = store.capture_observation(ObservationInput {
        event_id: "evt-meta-inherited".to_owned(),
        session_id: "sess-meta".to_owned(),
        content: "Inherited session context is already stored.".to_owned(),
        role: "assistant".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: LifecycleInput {
            sensitivity: Some(Sensitivity::Public),
            ..LifecycleInput::default()
        },
        context: WorkContext::default(),
    })?;
    assert_eq!(inherited.observation.context, started.context);
    assert_eq!(
        inherited.observation.lifecycle.sensitivity,
        Sensitivity::Private
    );
    assert_eq!(inherited.observation.event_id, "evt-meta-inherited");
    let captured = store.capture_observation(captured_input)?;
    assert_metadata_redacted(&captured.observation.context);
    assert_eq!(
        captured.observation.lifecycle.sensitivity,
        Sensitivity::Secret
    );
    let partial = store.capture_observation(ObservationInput {
        event_id: "evt-meta-partial".to_owned(),
        session_id: "sess-meta".to_owned(),
        content: "A partial context does not inherit the session branch.".to_owned(),
        role: "assistant".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: LifecycleInput::default(),
        context: WorkContext {
            commands: vec![SK.to_owned()],
            ..WorkContext::default()
        },
    })?;
    assert!(partial.observation.context.branch.is_none());
    assert_eq!(
        partial.observation.context.commands,
        vec!["[REDACTED]".to_owned()]
    );
    assert_eq!(
        partial.observation.lifecycle.sensitivity,
        Sensitivity::Secret
    );
    let summary = store.end_session("sess-meta")?;
    assert_eq!(summary.lifecycle.sensitivity, Sensitivity::Secret);
    assert_eq!(summary.session_id, "sess-meta");
    assert!(
        summary
            .source_event_ids
            .contains(&"evt-meta-capture".to_owned())
    );
    assert!(!summary.content.contains(SK));
    assert!(summary.next_steps.iter().all(|step| !step.contains(SK)));
    let target = store.remember(
        "Ordinary correction target.",
        Vec::new(),
        WorkContext {
            repository: Some("cairn-demo".to_owned()),
            ..WorkContext::default()
        },
    )?;
    let corrected = store.correct(
        &target.memory_id,
        "Ordinary correction note.",
        vec!["token=synthetic-concept".to_owned()],
        secret_context,
        LifecycleInput {
            sensitivity: Some(Sensitivity::Public),
            ..LifecycleInput::default()
        },
    )?;
    assert_metadata_redacted(&corrected.context);
    assert_eq!(corrected.lifecycle.sensitivity, Sensitivity::Secret);
    assert_eq!(
        corrected.supersedes.as_deref(),
        Some(target.memory_id.as_str())
    );
    assert!(!corrected.content.contains("synthetic-concept"));
    assert!(!corrected.content.contains(SK));
    let persisted = fs::read_to_string(&path)?;
    let exported = serde_json::to_string(&store.export())?;
    for secret in [SK, BODY, "BEGIN RSA PRIVATE KEY", "synthetic-concept"] {
        assert!(!persisted.contains(secret), "store still has {secret}");
        assert!(!exported.contains(secret), "export still has {secret}");
    }
    assert!(persisted.contains("sk-looking-name"));
    assert!(persisted.contains("evt-meta-capture"));
    assert!(persisted.contains("café"));
    drop(store);
    let mut reopened = MemoryStore::open(&path)?;
    let session = reopened
        .sessions()
        .find(|session| session.session_id == "sess-meta")
        .expect("session reopened");
    assert_metadata_redacted(&session.context);
    assert!(!fs::read_to_string(&path)?.contains(SK));

    let bundle = ExportBundle {
        format: EXPORT_FORMAT.to_owned(),
        version: EXPORT_VERSION,
        exported_at: "10".to_owned(),
        sessions: vec![Session {
            session_id: "sess-import-meta".to_owned(),
            person_id: None,
            context: metadata_secret_context(),
            started_at: "10".to_owned(),
            ended_at: None,
        }],
        observations: vec![Observation {
            event_id: "evt-import-meta".to_owned(),
            session_id: "sess-import-meta".to_owned(),
            content: "ordinary import observation".to_owned(),
            role: "assistant".to_owned(),
            source: "test".to_owned(),
            importance: None,
            lifecycle: Lifecycle {
                sensitivity: Sensitivity::Public,
                ..Lifecycle::default()
            },
            scope: MemoryScope::Personal,
            context: metadata_secret_context(),
            captured_at: "10".to_owned(),
        }],
        memories: vec![Memory {
            fields: Default::default(),
            memory_id: "memory-import-meta".to_owned(),
            session_id: "sess-import-meta".to_owned(),
            content: "ordinary import memory".to_owned(),
            source_event_ids: vec!["evt-import-meta".to_owned()],
            context: metadata_secret_context(),
            lifecycle: Lifecycle {
                sensitivity: Sensitivity::Public,
                ..Lifecycle::default()
            },
            kind: MemoryKind::Explicit,
            scope: MemoryScope::Global,
            project_id: None,
            next_steps: vec!["next step SK-synthetic-example".to_owned()],
            unresolved_work: vec![
                "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END RSA PRIVATE KEY-----\n"
                    .to_owned(),
            ],
            supersedes: None,
            contradicts: Vec::new(),
            links: Vec::new(),
            promoted_from: Some("memory-source-1".to_owned()),
            promotion_reason: Some("because SK-synthetic-example".to_owned()),
            verified_commit: Some("abc1234".to_owned()),
            published_by: None, changed_by: None, changed_at: None,
            created_at: "10".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            helpful_count: 0,
            stale_count: 0,
            feedback: Vec::new(),
        }],
        forgotten_ids: vec![],
    };
    let imported = reopened.import_bundle(bundle.clone())?;
    assert_eq!(imported.imported_sessions, 1);
    assert_eq!(imported.imported_observations, 1);
    assert_eq!(imported.imported_memories, 1);
    let again = reopened.import_bundle(bundle)?;
    assert_eq!(again.imported_memories, 0);
    let import_file = fs::read_to_string(&path)?;
    let import_export = serde_json::to_string(&reopened.export())?;
    for secret in [SK, BODY, "BEGIN RSA PRIVATE KEY"] {
        assert!(
            !import_file.contains(secret),
            "import store still has {secret}"
        );
        assert!(
            !import_export.contains(secret),
            "import export still has {secret}"
        );
    }
    let imported_memory = reopened
        .export()
        .memories
        .into_iter()
        .find(|memory| memory.memory_id == "memory-import-meta")
        .expect("imported memory");
    assert_eq!(imported_memory.lifecycle.sensitivity, Sensitivity::Public);
    assert_eq!(imported_memory.promoted_from.as_deref(), None);
    assert_eq!(imported_memory.verified_commit.as_deref(), Some("abc1234"));
    assert_eq!(
        imported_memory.source_event_ids,
        vec!["evt-import-meta".to_owned()]
    );
    assert_metadata_redacted(&imported_memory.context);
    assert!(
        imported_memory
            .next_steps
            .iter()
            .all(|step| !step.contains(SK))
    );
    assert!(
        imported_memory
            .next_steps
            .iter()
            .any(|step| step.contains(REDACTED))
    );
    assert!(
        imported_memory
            .unresolved_work
            .iter()
            .all(|item| !item.contains(BODY))
    );
    assert_eq!(imported_memory.promotion_reason.as_deref(), None);
    let imported_session = reopened
        .sessions()
        .find(|session| session.session_id == "sess-import-meta")
        .expect("imported session");
    assert_metadata_redacted(&imported_session.context);
    fs::remove_file(&path)?;

    let old_path = temp_store_path("metadata-old");
    let old_store = serde_json::json!({
        "version": 1,
        "observations": [{
            "event_id": "evt-old-meta",
            "session_id": "sess-old-meta",
            "content": "ordinary old note",
            "role": "assistant",
            "source": "test",
            "importance": null,
            "context": {"commands": [SK], "branch": "feat/auth"},
            "captured_at": "1"
        }],
        "memories": [{
            "memory_id": "memory-old-meta",
            "session_id": "sess-old-meta",
            "content": "ordinary old memory",
            "source_event_ids": ["evt-old-meta"],
            "context": {"commands": [SK], "branch": "feat/auth"},
            "created_at": "1"
        }]
    });
    fs::write(&old_path, serde_json::to_string(&old_store)?)?;
    let before = fs::read_to_string(&old_path)?;
    let mut old = MemoryStore::open(&old_path)?;
    assert_eq!(fs::read_to_string(&old_path)?, before);
    let old_observation = old
        .observations()
        .find(|observation| observation.event_id == "evt-old-meta")
        .expect("old observation");
    assert_eq!(old_observation.context.commands, vec![SK.to_owned()]);
    assert_eq!(old_observation.context.branch.as_deref(), Some("feat/auth"));
    let inherited_correction = old.correct(
        "memory-old-meta",
        "Ordinary correction of an old memory.",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput {
            sensitivity: Some(Sensitivity::Public),
            ..LifecycleInput::default()
        },
    )?;
    assert_eq!(
        inherited_correction.context.commands,
        vec!["[REDACTED]".to_owned()]
    );
    assert_eq!(
        inherited_correction.lifecycle.sensitivity,
        Sensitivity::Secret
    );
    assert_eq!(
        inherited_correction.supersedes.as_deref(),
        Some("memory-old-meta")
    );
    assert_eq!(
        inherited_correction.context.branch.as_deref(),
        Some("feat/auth")
    );
    let still_old = old
        .export()
        .memories
        .into_iter()
        .find(|memory| memory.memory_id == "memory-old-meta")
        .expect("old memory");
    assert_eq!(still_old.context.commands, vec![SK.to_owned()]);
    assert_eq!(still_old.lifecycle.sensitivity, Sensitivity::Private);
    let old_file = fs::read_to_string(&old_path)?;
    assert!(old_file.contains(SK));
    let new_only = serde_json::to_string(&inherited_correction)?;
    assert!(!new_only.contains(SK));
    assert!(new_only.contains("memory-old-meta"));
    fs::remove_file(old_path)?;
    Ok(())
}
