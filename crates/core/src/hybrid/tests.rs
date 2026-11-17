use super::*;
use crate::{LifecycleInput, MemoryKind, RecallFilter, WorkContext, build_explicit_memory_record};

#[test]
fn rrf_weights_pinning_and_one_hop_are_explicit() {
    let note = |id: &str, kind| {
        let mut m = build_explicit_memory_record(
            "body",
            vec![],
            WorkContext::default(),
            LifecycleInput::default(),
            kind,
        )
        .unwrap();
        m.memory_id = id.to_owned();
        m
    };
    let mut memories = vec![
        note("a", MemoryKind::Capture),
        note("b", MemoryKind::Fact),
        note("c", MemoryKind::Rule),
        note("d", MemoryKind::Fact),
        note("e", MemoryKind::Fact),
    ];
    memories[3].fields.pinned = true;
    memories[4].fields.evidence = vec!["d".to_owned()];
    let hits = hybrid_rank(
        &memories,
        "query",
        &RecallFilter::default(),
        Some(vec![
            ("a".into(), 4.0),
            ("b".into(), 3.0),
            ("c".into(), 2.0),
            ("d".into(), 1.0),
        ]),
        vec![],
    )
    .unwrap();
    let score = |id| hits.iter().find(|(key, _)| key == id).unwrap().1;
    assert!((score("a") - 1.0 / 61.0).abs() < 1e-7);
    assert!((score("b") - 2.0 / 62.0).abs() < 1e-7);
    assert!((score("c") - 3.0 / 63.0).abs() < 1e-7);
    assert!((score("d") - 4.0 / 64.0).abs() < 1e-7);
    assert!((score("e") - 2.0 / 61.0).abs() < 1e-7);
    memories.push(note("f", MemoryKind::Fact));
    memories[5].fields.evidence = vec!["e".into()];
    let hits = hybrid_rank(
        &memories,
        "query",
        &RecallFilter::default(),
        Some(vec![("d".into(), 1.0)]),
        vec![],
    )
    .unwrap();
    assert!(!hits.iter().any(|(id, _)| id == "f"));
}

#[test]
fn a_typed_link_hops_once_with_the_same_weight_as_evidence() {
    let note = |id: &str| {
        let mut memory = build_explicit_memory_record(
            "body",
            vec![],
            WorkContext::default(),
            LifecycleInput::default(),
            MemoryKind::Fact,
        )
        .unwrap();
        memory.memory_id = id.to_owned();
        memory
    };
    let mut seed = note("seed");
    let mut neighbour = note("neighbour");
    seed.links.push(crate::MemoryLink {
        relation: crate::LinkRelation::Causes,
        target_id: "neighbour".to_owned(),
    });
    neighbour.contradicts.push("seed".to_owned());
    let hits = hybrid_rank(
        &[seed.clone(), neighbour.clone(), note("alone")],
        "query",
        &RecallFilter::default(),
        Some(vec![("seed".into(), 1.0)]),
        vec![],
    )
    .unwrap();
    let score = |id| {
        hits.iter()
            .find(|(key, _)| key == id)
            .map(|(_, score)| *score)
    };
    assert!((score("seed").unwrap() - 2.0 / 61.0).abs() < 1e-6);
    assert!((score("neighbour").unwrap() - 2.0 / 61.0).abs() < 1e-6);
    assert!(score("alone").is_none());
    seed.links.clear();
    let shaded = hybrid_rank(
        &[seed, neighbour],
        "query",
        &RecallFilter::default(),
        Some(vec![("seed".into(), 1.0)]),
        vec![],
    )
    .unwrap();
    assert!(shaded.iter().all(|(id, _)| id != "neighbour"));
}

#[test]
fn shared_source_ids_do_not_cross_the_person_namespace() {
    let mut a = build_explicit_memory_record(
        "body",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
        MemoryKind::Fact,
    )
    .unwrap();
    a.memory_id = "a".into();
    a.source_event_ids = vec!["event-1".into()];
    a.fields.author = Some("alice".into());
    let mut b = a.clone();
    b.memory_id = "b".into();
    b.fields.author = Some("bob".into());
    let hits = hybrid_rank(
        &[a.clone(), b.clone()],
        "query",
        &RecallFilter::default(),
        Some(vec![("a".into(), 1.0)]),
        vec![],
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    b.fields.author = a.fields.author.clone();
    assert_eq!(
        hybrid_rank(
            &[a, b],
            "query",
            &RecallFilter::default(),
            Some(vec![("a".into(), 1.0)]),
            vec![]
        )
        .unwrap()
        .len(),
        2
    );
}
