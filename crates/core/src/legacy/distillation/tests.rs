use super::*;
use crate::legacy::test_support::observation_for;
use crate::legacy::{MemoryScope, WorkContext, derive_session_memory, normalize_observation};

#[test]
fn extraction_is_explicit_bounded_and_does_not_invent_fields() {
    let source = normalize_observation(&observation_for(
        "distill",
        "source",
        "ordinary decision prose\nfAcT: one writer\nStatement: a stated fact\nSource: verified.rs\nUnlabelled prose ends metadata\nReason: must not attach\nRule: keep writes bounded\nProcedure: check writes\nStep: cargo test\nWhen: after edits",
    )).unwrap();
    let summary = derive_session_memory(
        None,
        "distill",
        &WorkContext::default(),
        std::slice::from_ref(&source),
    )
    .unwrap();
    let notes = distill_session_notes(&summary, &[source]);
    assert!(notes.iter().all(|note| note.kind != MemoryKind::Handoff));
    assert_eq!(notes.len(), 3);
    let fact = notes
        .iter()
        .find(|note| note.kind == MemoryKind::Fact)
        .unwrap();
    assert_eq!(fact.fields.statement.as_deref(), Some("a stated fact"));
    assert_eq!(fact.fields.source.as_deref(), Some("verified.rs"));
    assert!(fact.fields.reason.is_empty());
    assert_eq!(fact.fields.evidence, ["source"]);
    assert_eq!(fact.scope, MemoryScope::Personal);
    assert!(
        notes
            .iter()
            .find(|note| note.kind == MemoryKind::Rule)
            .unwrap()
            .fields
            .rule
            .is_none(),
        "a type prefix is not a typed field"
    );
    let procedure = notes
        .iter()
        .find(|note| note.kind == MemoryKind::Procedure)
        .unwrap();
    assert_eq!(procedure.fields.steps, ["cargo test"]);
    assert_eq!(procedure.fields.when.as_deref(), Some("after edits"));

    let many = normalize_observation(&observation_for(
        "distill",
        "many",
        &"Fact: bounded\n".repeat(100),
    ))
    .unwrap();
    let notes = distill_session_notes(&summary, &[many]);
    assert_eq!(notes.len(), MAX_TYPED_NOTES);
    let ids: std::collections::HashSet<_> = notes.iter().map(|note| &note.memory_id).collect();
    assert_eq!(ids.len(), notes.len());
}

#[test]
fn derived_ids_cannot_collide_with_session_summary_ids() {
    let source =
        normalize_observation(&observation_for("work", "source", "Fact: one writer")).unwrap();
    let summary = derive_session_memory(
        None,
        "work",
        &WorkContext::default(),
        std::slice::from_ref(&source),
    )
    .unwrap();
    let notes = distill_session_notes(&summary, std::slice::from_ref(&source));
    for session in ["work-handoff", "work-distilled-6:source:0"] {
        let other = derive_session_memory(
            None,
            session,
            &WorkContext::default(),
            std::slice::from_ref(&source),
        )
        .unwrap();
        assert!(
            notes.iter().all(|note| note.memory_id != other.memory_id),
            "session {session} collides with a derived note"
        );
    }
}
