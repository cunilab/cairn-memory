use crate::legacy::capture::ObservationInput;
use crate::legacy::lifecycle::{LifecycleInput, MemoryKind, MemoryScope, Sensitivity};
use crate::legacy::memories::{Memory, build_explicit_memory_record};
use crate::legacy::privacy::REDACTED;
use crate::legacy::recall::RecallResult;
use crate::legacy::store::MemoryStore;
use crate::legacy::transfer::{ExportBundle, ImportResult};
use crate::legacy::types::{StoreError, WorkContext, now_seconds};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn temp_store_path(test_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cairn-{test_name}-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

pub(crate) fn context() -> WorkContext {
    WorkContext {
        project_id: None,
        repository: Some("cairn-demo".to_owned()),
        branch: Some("feat/auth".to_owned()),
        worktree: Some("wt-auth".to_owned()),
        commit: Some("abc1234".to_owned()),
        files: vec!["src/auth/session.rs".to_owned()],
        symbols: vec!["SessionMiddleware::validate_cookie".to_owned()],
        commands: vec!["cargo test auth_session".to_owned()],
        tests: vec!["missing_signature_is_rejected".to_owned()],
        services: vec!["auth".to_owned()],
        handoffs: Vec::new(),
        runbooks: Vec::new(),
    }
}

pub(crate) fn observation(event_id: &str, content: &str) -> ObservationInput {
    observation_for("sess-auth-001", event_id, content)
}

pub(crate) fn observation_for(session_id: &str, event_id: &str, content: &str) -> ObservationInput {
    ObservationInput {
        event_id: event_id.to_owned(),
        session_id: session_id.to_owned(),
        content: content.to_owned(),
        role: "assistant".to_owned(),
        source: "test".to_owned(),
        importance: Some("high".to_owned()),
        lifecycle: LifecycleInput::default(),
        context: context(),
    }
}

// CM-AUDIT-012: `supersedes` is stored provenance. It hides its target only when the
// successor does not belong to a different person than the target.
pub(crate) fn linked_memory(label: &str, supersedes: &str) -> Memory {
    let mut memory = build_explicit_memory_record(
        &format!("betalink {label} unrelated"),
        Vec::new(),
        WorkContext::default(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )
    .expect("linked memory");
    memory.memory_id = format!("memory-link-{label}");
    memory.source_event_ids = vec![format!("event-link-{label}")];
    memory.supersedes = Some(supersedes.to_owned());
    memory
}

pub(crate) fn import_memories(
    store: &mut MemoryStore,
    token: &str,
    memories: Vec<Memory>,
) -> Result<(), StoreError> {
    let mut bundle = store.export_for_account(token)?;
    bundle.sessions.clear();
    bundle.observations.clear();
    bundle.forgotten_ids.clear();
    bundle.memories = memories;
    store.import_for_account(token, bundle)?;
    Ok(())
}

pub(crate) fn ids_of(results: &[RecallResult]) -> Vec<&str> {
    let mut ids = results
        .iter()
        .map(|result| result.memory_id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

pub(crate) fn remember_owned(
    store: &mut MemoryStore,
    token: &str,
    content: &str,
) -> Result<Memory, StoreError> {
    store.remember_for_account(
        token,
        content,
        Vec::new(),
        WorkContext::default(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )
}

// CM-AUDIT-013: an account import applies a tombstone only for ids the caller owns or
// brings as rows. A foreign-owned id rejects the import; other ids are ignored.
pub(crate) fn import_forgotten(
    store: &mut MemoryStore,
    token: &str,
    memories: Vec<Memory>,
    forgotten: &[&str],
) -> Result<ImportResult, StoreError> {
    let mut bundle = store.export_for_account(token)?;
    bundle.sessions.clear();
    bundle.observations.clear();
    bundle.memories = memories;
    bundle.forgotten_ids = forgotten.iter().map(|id| (*id).to_owned()).collect();
    store.import_for_account(token, bundle)
}

// CM-AUDIT-014 finding 1: a project-linked row in an account import must refer to a
// project that account owns, and a stored foreign row stays out of the owner's views.
pub(crate) fn project_row(id: &str, project_id: &str, content: &str) -> Memory {
    let mut memory = build_explicit_memory_record(
        content,
        Vec::new(),
        context(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )
    .expect("project row");
    memory.memory_id = id.to_owned();
    memory.source_event_ids = vec![format!("event-{id}")];
    memory.scope = MemoryScope::Project;
    memory.project_id = Some(project_id.to_owned());
    memory
}

pub(crate) fn owned_project(
    store: &mut MemoryStore,
    token: &str,
    content: &str,
) -> Result<(String, Memory), StoreError> {
    let project = store.register_project(token, "cairn-demo", "Demo", None)?;
    let promoted = save_project_memory(store, token, &project.project_id, content, context())?;
    Ok((project.project_id, promoted))
}

pub(crate) fn remember_with_context(
    store: &mut MemoryStore,
    token: &str,
    content: &str,
) -> Result<Memory, StoreError> {
    store.remember_for_account(
        token,
        content,
        Vec::new(),
        context(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )
}

// CM-AUDIT-013 findings 2 and 3: an account forgets only ids it owns, and a forget
// cascades only the source ids that account owns.
pub(crate) fn assert_unknown_id(error: &StoreError, id: &str) {
    assert!(
        matches!(error, StoreError::Validation(text) if text == &format!("unknown memory or event: {id}")),
        "unexpected error {error}"
    );
}

pub(crate) fn empty_capture(session_id: &str, event_id: &str, content: &str) -> ObservationInput {
    ObservationInput {
        context: WorkContext::default(),
        ..observation_for(session_id, event_id, content)
    }
}

/// Makes the account a project member the way the committed project file does: one
/// capture that carries the project id.
pub(crate) fn join_project(
    store: &mut MemoryStore,
    token: &str,
    project_id: &str,
) -> Result<(), StoreError> {
    store.join_project(token, project_id)?;
    Ok(())
}

pub(crate) fn observation_with_sensitivity(
    event_id: &str,
    content: &str,
    sensitivity: Option<Sensitivity>,
) -> ObservationInput {
    let mut input = observation(event_id, content);
    input.lifecycle.sensitivity = sensitivity;
    input
}

pub(crate) fn metadata_secret_context() -> WorkContext {
    WorkContext {
        project_id: None,
        repository: Some("cairn-demo".to_owned()),
        branch: Some("sk-looking-name".to_owned()),
        worktree: Some("wt-auth".to_owned()),
        commit: Some("abc1234".to_owned()),
        files: vec!["src/lib.rs".to_owned()],
        symbols: vec!["MemoryStore::open".to_owned()],
        commands: vec!["echo café SK-synthetic-example".to_owned()],
        tests: vec!["redacts_human_metadata".to_owned()],
        services: vec!["cairn".to_owned()],
        handoffs: vec!["handoff stays".to_owned()],
        runbooks: vec![
            "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END RSA PRIVATE KEY-----\n"
                .to_owned(),
        ],
    }
}

pub(crate) fn assert_metadata_redacted(context: &WorkContext) {
    const SK: &str = "SK-synthetic-example";
    const BODY: &str = "c3ludGhldGljLWtleS1ib2R5";
    let rendered = serde_json::to_string(context).expect("context json");
    assert!(!rendered.contains(SK), "context still has {SK}: {rendered}");
    assert!(
        !rendered.contains(BODY),
        "context still has {BODY}: {rendered}"
    );
    assert!(!rendered.contains("BEGIN RSA PRIVATE KEY"), "{rendered}");
    assert_eq!(context.repository.as_deref(), Some("cairn-demo"));
    assert_eq!(context.branch.as_deref(), Some("sk-looking-name"));
    assert_eq!(context.worktree.as_deref(), Some("wt-auth"));
    assert_eq!(context.commit.as_deref(), Some("abc1234"));
    assert_eq!(context.files, vec!["src/lib.rs".to_owned()]);
    assert_eq!(context.symbols, vec!["MemoryStore::open".to_owned()]);
    assert_eq!(context.commands, vec!["echo café [REDACTED]".to_owned()]);
    assert_eq!(context.tests, vec!["redacts_human_metadata".to_owned()]);
    assert_eq!(context.services, vec!["cairn".to_owned()]);
    assert_eq!(context.handoffs, vec!["handoff stays".to_owned()]);
    assert!(
        context
            .runbooks
            .iter()
            .all(|value| value.contains(REDACTED))
    );
    assert!(context.commands[0].contains("café"));
}

pub(crate) struct HeldStoreDir {
    parent: PathBuf,
    held: PathBuf,
    released: bool,
}

impl HeldStoreDir {
    pub(crate) fn block(store_path: &Path) -> Self {
        let parent = store_path
            .parent()
            .expect("store path has a parent")
            .to_path_buf();
        let held = parent.with_file_name(format!(
            "{}.held",
            parent
                .file_name()
                .expect("store dir name")
                .to_string_lossy()
        ));
        fs::rename(&parent, &held).expect("move store dir aside");
        fs::write(&parent, b"not a directory").expect("replace store dir with a file");
        Self {
            parent,
            held,
            released: false,
        }
    }

    pub(crate) fn release(&mut self) {
        if self.released {
            return;
        }
        if self.parent.is_file() {
            fs::remove_file(&self.parent).expect("remove blocking file");
        }
        if self.held.is_dir() && !self.parent.exists() {
            fs::rename(&self.held, &self.parent).expect("restore store dir");
        }
        self.released = true;
    }
}

impl Drop for HeldStoreDir {
    fn drop(&mut self) {
        self.release();
    }
}

pub(crate) fn temp_store_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cairn-{test_name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp store dir");
    dir
}

// A bundle with a session, a live row, an already-expired row, a tombstoned row and a memory.
pub(crate) fn import_fixture() -> Result<ExportBundle, StoreError> {
    let path = temp_store_path("import-fixture");
    let mut source = MemoryStore::open(&path)?;
    source.start_session("imp-session", context())?;
    for id in ["imp-live", "imp-expired", "imp-gone"] {
        source.capture_observation(observation_for("imp-session", id, "importfact"))?;
    }
    source.remember("importmemory fact", Vec::new(), context())?;
    let mut bundle = source.export();
    bundle
        .observations
        .iter_mut()
        .find(|observation| observation.event_id == "imp-expired")
        .expect("expired row")
        .lifecycle
        .expires_at = Some("1".to_owned());
    bundle.forgotten_ids = vec!["imp-gone".to_owned()];
    fs::remove_file(path)?;
    Ok(bundle)
}

pub(crate) type ImportSnapshot = (
    Vec<String>,
    Vec<String>,
    usize,
    Vec<String>,
    usize,
    usize,
    usize,
);

pub(crate) fn import_snapshot(store: &MemoryStore) -> ImportSnapshot {
    let mut forgotten = store
        .state
        .forgotten_ids
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    forgotten.sort();
    (
        store.observations().map(|o| o.event_id.clone()).collect(),
        store
            .state
            .memories
            .iter()
            .map(|m| m.memory_id.clone())
            .collect(),
        store.state.sessions.len(),
        forgotten,
        store.state.observation_owners.len(),
        store.state.memory_owners.len(),
        store.state.sync_deletions.len(),
    )
}

pub(crate) fn owner_pairs(store: &MemoryStore) -> Vec<(String, String)> {
    store
        .state
        .observation_owners
        .iter()
        .map(|record| (record.event_id.clone(), record.person_id.clone()))
        .collect()
}

pub(crate) fn owner_context_for_claim() -> WorkContext {
    WorkContext {
        repository: Some("owner-repo".to_owned()),
        branch: Some("owner-branch".to_owned()),
        ..WorkContext::default()
    }
}

pub(crate) fn age_observations(store: &mut MemoryStore, session_id: &str, secs: u64) {
    let at = (now_seconds() - secs).to_string();
    for observation in &mut store.state.observations {
        if observation.session_id == session_id {
            observation.captured_at.clone_from(&at);
        }
    }
}

pub(crate) fn ended(store: &MemoryStore, person_id: &str, session_id: &str) -> bool {
    store
        .state
        .sessions
        .iter()
        .find(|s| s.session_id == session_id && s.person_id.as_deref() == Some(person_id))
        .is_some_and(|s| s.ended_at.is_some())
}

pub(crate) fn summaries<'a>(store: &'a MemoryStore, session_id: &str) -> Vec<&'a Memory> {
    store
        .state
        .memories
        .iter()
        .filter(|m| m.kind == MemoryKind::SessionSummary && m.session_id == session_id)
        .collect()
}

pub(crate) fn repo_context(repository: &str, branch: Option<&str>) -> WorkContext {
    WorkContext {
        repository: Some(repository.to_owned()),
        branch: branch.map(ToOwned::to_owned),
        ..WorkContext::default()
    }
}

pub(crate) fn capture_in(
    store: &mut MemoryStore,
    token: &str,
    event_id: &str,
    context: WorkContext,
) -> Result<(), StoreError> {
    let mut input = observation_for("reg-session", event_id, "regfact");
    input.context = context;
    store.capture_for_account(token, input)?;
    Ok(())
}

pub(crate) fn context_from_fixture(value: &Value) -> WorkContext {
    WorkContext {
        project_id: None,
        repository: value["repository"].as_str().map(ToOwned::to_owned),
        branch: value["branch"].as_str().map(ToOwned::to_owned),
        worktree: value["worktree"].as_str().map(ToOwned::to_owned),
        commit: value["commit"].as_str().map(ToOwned::to_owned),
        files: value["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        symbols: context_items(value, "symbols"),
        commands: context_items(value, "commands"),
        tests: context_items(value, "tests"),
        services: context_items(value, "services"),
        handoffs: context_items(value, "handoffs"),
        runbooks: context_items(value, "runbooks"),
    }
}

pub(crate) fn with_project_id(mut context: WorkContext, project_id: &str) -> WorkContext {
    context.project_id = Some(project_id.to_owned());
    context
}

pub(crate) fn context_items(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn save_project_memory(
    store: &mut MemoryStore,
    token: &str,
    project_id: &str,
    content: &str,
    context: WorkContext,
) -> Result<Memory, StoreError> {
    store.save_for_account(token, serde_json::from_value(serde_json::json!({"scope":"project", "title":content,"summary":content,"reason":"verified fixture","context":WorkContext { project_id:Some(project_id.to_owned()), ..context }}))?)
}
