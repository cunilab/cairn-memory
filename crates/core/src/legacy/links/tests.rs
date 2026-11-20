use super::*;
use crate::legacy::memories::Memory;
use crate::legacy::recall::RecallFilter;
use crate::legacy::test_support::{join_project, save_project_memory, temp_store_path};
use crate::legacy::types::WorkContext;
use crate::legacy::{MemoryStore, build_saved_memory};
use serde_json::json;
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

fn blank(id: &str, created_at: &str) -> Memory {
    let mut memory = build_saved_memory(
        serde_json::from_value(json!({
            "scope": "personal",
            "kind": "fact",
            "title": id,
            "summary": id,
            "reason": "verified"
        }))
        .unwrap(),
        "alice",
    )
    .unwrap();
    memory.memory_id = id.to_owned();
    memory.created_at = created_at.to_owned();
    memory
}

#[test]
fn link_permissions_follow_the_source_row_and_stay_in_scope() {
    let path = temp_store_path("memory-links");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let member = store
        .register_account("member@example.test", "correct horse battery staple")
        .unwrap();
    let source = save(&mut store, &owner.token, "personal", "seedtoken");
    let target = save(&mut store, &owner.token, "personal", "othertoken");
    let global = save(&mut store, &owner.token, "global", "globaltoken");
    let hidden = store
        .link_memories(
            &member.token,
            &source.memory_id,
            LinkRelation::Causes,
            &target.memory_id,
        )
        .unwrap_err();
    assert!(hidden.to_string().contains("unknown memory"));
    let crossed = store
        .link_memories(
            &owner.token,
            &source.memory_id,
            LinkRelation::Causes,
            &global.memory_id,
        )
        .unwrap_err();
    assert!(crossed.to_string().contains("same scope"));
    let itself = store
        .link_memories(
            &owner.token,
            &source.memory_id,
            LinkRelation::Relates,
            &source.memory_id,
        )
        .unwrap_err();
    assert!(itself.to_string().contains("itself"));
    let linked = store
        .link_memories(
            &owner.token,
            &source.memory_id,
            LinkRelation::Causes,
            &target.memory_id,
        )
        .unwrap();
    assert_eq!(linked.links.len(), 1);
    store
        .link_memories(
            &owner.token,
            &source.memory_id,
            LinkRelation::Causes,
            &target.memory_id,
        )
        .unwrap();
    let reopened = MemoryStore::open(&path).unwrap();
    let stored = reopened
        .memories_for_account(&owner.token)
        .unwrap()
        .into_iter()
        .find(|memory| memory.memory_id == source.memory_id)
        .unwrap();
    assert_eq!(stored.links.len(), 1);
    assert_eq!(stored.links[0].relation, LinkRelation::Causes);
    let mut store = reopened;
    let removed = store
        .unlink_memories(
            &owner.token,
            &source.memory_id,
            LinkRelation::Causes,
            &target.memory_id,
        )
        .unwrap();
    assert!(removed.links.is_empty());

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
        .link_memories(
            &member.token,
            &left.memory_id,
            LinkRelation::Fixes,
            &right.memory_id,
        )
        .unwrap_err();
    assert!(refused.to_string().contains("author or project owner"));
    store
        .link_memories(
            &owner.token,
            &left.memory_id,
            LinkRelation::Fixes,
            &right.memory_id,
        )
        .unwrap();
    let owner_global = save(&mut store, &owner.token, "global", "owner-global");
    let second_global = save(&mut store, &owner.token, "global", "second-global");
    store
        .link_memories(
            &owner.token,
            &owner_global.memory_id,
            LinkRelation::DependsOn,
            &second_global.memory_id,
        )
        .unwrap();
    let not_admin = store
        .unlink_memories(
            &member.token,
            &owner_global.memory_id,
            LinkRelation::DependsOn,
            &second_global.memory_id,
        )
        .unwrap_err();
    assert!(not_admin.to_string().contains("administrator"));
    fs::remove_file(path).unwrap();
}

#[test]
fn graph_omits_hidden_ends_and_drops_the_least_connected_oldest_nodes() {
    let mut memories = vec![blank("hub", "5"), blank("old", "1"), blank("new", "9")];
    memories[0].links.push(MemoryLink {
        relation: LinkRelation::Relates,
        target_id: "new".to_owned(),
    });
    memories[0].links.push(MemoryLink {
        relation: LinkRelation::Causes,
        target_id: "missing".to_owned(),
    });
    memories[0].contradicts.push("new".to_owned());
    memories[2].supersedes = Some("old".to_owned());
    let full = build_memory_graph(&memories, &GraphFilter::default(), 120);
    assert!(
        full.edges
            .iter()
            .any(|edge| edge.relation == GraphRelation::Supersedes && edge.target_id == "old")
    );
    let graph = build_memory_graph(&memories, &GraphFilter::default(), 2);
    assert!(graph.truncated);
    assert_eq!(graph.node_cap, 2);
    let ids = graph
        .nodes
        .iter()
        .map(|node| node.memory_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["hub", "new"]);
    assert!(graph.edges.iter().all(|edge| edge.target_id != "missing"));
    assert!(graph.edges.iter().all(|edge| edge.target_id != "old"));
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.relation == GraphRelation::Contradicts)
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.relation == GraphRelation::Relates)
    );
    let causes = build_memory_graph(
        &memories,
        &GraphFilter {
            relation: Some(GraphRelation::Causes),
            ..GraphFilter::default()
        },
        120,
    );
    assert!(causes.nodes.is_empty());
    assert!(!causes.truncated);

    let path = temp_store_path("memory-graph");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .unwrap();
    let source = save(&mut store, &owner.token, "personal", "visible-source");
    let target = save(&mut store, &owner.token, "personal", "visible-target");
    let foreign = save(&mut store, &other.token, "personal", "foreign-secret");
    store
        .link_memories(
            &owner.token,
            &source.memory_id,
            LinkRelation::Relates,
            &target.memory_id,
        )
        .unwrap();
    store
        .declare_contradiction(&owner.token, &source.memory_id, &target.memory_id)
        .unwrap();
    store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.memory_id == source.memory_id)
        .unwrap()
        .links
        .push(MemoryLink {
            relation: LinkRelation::Causes,
            target_id: foreign.memory_id.clone(),
        });
    let visible = store
        .graph_for_account(&owner.token, &GraphFilter::default(), None)
        .unwrap();
    assert!(
        visible
            .nodes
            .iter()
            .all(|node| node.memory_id != foreign.memory_id)
    );
    assert!(visible.edges.iter().all(|edge| {
        edge.source_id != foreign.memory_id && edge.target_id != foreign.memory_id
    }));
    assert!(
        visible
            .edges
            .iter()
            .any(|edge| edge.relation == GraphRelation::Contradicts)
    );
    store
        .supersede_memory(&owner.token, &source.memory_id, &target.memory_id)
        .unwrap();
    let hidden = store
        .graph_for_account(&owner.token, &GraphFilter::default(), Some(120))
        .unwrap();
    assert!(
        hidden
            .nodes
            .iter()
            .all(|node| node.memory_id != target.memory_id)
    );
    assert!(
        hidden
            .edges
            .iter()
            .all(|edge| edge.target_id != target.memory_id)
    );
    assert!(
        store
            .graph_for_account(&owner.token, &GraphFilter::default(), Some(119))
            .unwrap_err()
            .to_string()
            .contains("120")
    );
    assert!(
        store
            .graph_for_account(&owner.token, &GraphFilter::default(), Some(501))
            .is_err()
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn a_typed_link_recalls_one_hop_and_a_hidden_neighbour_does_not() {
    let path = temp_store_path("memory-link-recall");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .unwrap();
    let seed = save(&mut store, &owner.token, "personal", "seedtoken");
    let neighbour = save(&mut store, &owner.token, "personal", "othertoken");
    let alone = save(&mut store, &owner.token, "personal", "gammatoken");
    let foreign = save(&mut store, &other.token, "personal", "foreigntoken");
    store
        .link_memories(
            &owner.token,
            &seed.memory_id,
            LinkRelation::Causes,
            &neighbour.memory_id,
        )
        .unwrap();
    store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.memory_id == seed.memory_id)
        .unwrap()
        .links
        .push(MemoryLink {
            relation: LinkRelation::Fixes,
            target_id: foreign.memory_id.clone(),
        });
    let hits = store
        .search_for_account(&owner.token, "seedtoken", 10, &RecallFilter::default())
        .unwrap();
    let ids = hits
        .iter()
        .map(|hit| hit.memory_id.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&seed.memory_id.as_str()));
    assert!(ids.contains(&neighbour.memory_id.as_str()));
    assert!(!ids.contains(&alone.memory_id.as_str()));
    assert!(!ids.contains(&foreign.memory_id.as_str()));
    fs::remove_file(path).unwrap();
}

#[test]
fn graph_omits_a_cross_scope_link_between_visible_memories() {
    let mut personal = blank("personal", "1");
    let mut global = blank("global", "2");
    global.scope = crate::legacy::MemoryScope::Global;
    personal.links.push(MemoryLink {
        relation: LinkRelation::Causes,
        target_id: global.memory_id.clone(),
    });
    personal.links.push(MemoryLink {
        relation: LinkRelation::Relates,
        target_id: personal.memory_id.clone(),
    });
    let graph = build_memory_graph(&[personal, global], &GraphFilter::default(), 120);
    assert!(graph.edges.is_empty());
    assert_eq!(graph.nodes.len(), 2);
}

#[test]
fn import_drops_links_link_creation_would_reject() {
    let path = temp_store_path("import-links");
    let mut store = MemoryStore::open(&path).unwrap();
    let owner = store
        .bootstrap_account("owner@example.test", "correct horse battery staple")
        .unwrap();
    let other = store
        .register_account("other@example.test", "correct horse battery staple")
        .unwrap();
    let global = save(&mut store, &owner.token, "global", "quasarneedlexyz");
    let anchor = save(&mut store, &owner.token, "personal", "anchorplain");
    let foreign = save(&mut store, &other.token, "personal", "foreignplain");
    let mut incoming = anchor.clone();
    incoming.memory_id = "imported-cross-scope".to_owned();
    incoming.fields.title = "plain carton".to_owned();
    incoming.fields.summary = "plain carton".to_owned();
    incoming.fields.statement = Some("plain carton".to_owned());
    incoming.content = "plain carton".to_owned();
    incoming.links = vec![
        MemoryLink {
            relation: LinkRelation::Causes,
            target_id: global.memory_id.clone(),
        },
        MemoryLink {
            relation: LinkRelation::Relates,
            target_id: incoming.memory_id.clone(),
        },
        MemoryLink {
            relation: LinkRelation::Fixes,
            target_id: anchor.memory_id.clone(),
        },
        MemoryLink {
            relation: LinkRelation::Fixes,
            target_id: anchor.memory_id.clone(),
        },
        MemoryLink {
            relation: LinkRelation::DependsOn,
            target_id: "missing-target".to_owned(),
        },
        MemoryLink {
            relation: LinkRelation::Causes,
            target_id: foreign.memory_id.clone(),
        },
    ];
    let mut bundle = store.export_for_account(&owner.token).unwrap();
    bundle.sessions.clear();
    bundle.observations.clear();
    bundle.forgotten_ids.clear();
    bundle.memories = vec![incoming];
    let imported = store.import_for_account(&owner.token, bundle).unwrap();
    let reasons = imported
        .dropped_links
        .iter()
        .map(|link| (link.reason, link.target_id.as_str()))
        .collect::<Vec<_>>();
    assert!(reasons.contains(&(DroppedLinkReason::Scope, global.memory_id.as_str())));
    assert!(reasons.contains(&(DroppedLinkReason::SelfLink, "imported-cross-scope")));
    assert!(reasons.contains(&(DroppedLinkReason::Duplicate, anchor.memory_id.as_str())));
    assert!(reasons.contains(&(DroppedLinkReason::UnknownTarget, "missing-target")));
    assert!(reasons.contains(&(DroppedLinkReason::UnknownTarget, foreign.memory_id.as_str())));
    let stored = store
        .memories_for_account(&owner.token)
        .unwrap()
        .into_iter()
        .find(|memory| memory.memory_id == "imported-cross-scope")
        .unwrap();
    assert_eq!(stored.links.len(), 1);
    assert_eq!(stored.links[0].target_id, anchor.memory_id);
    let hits = store
        .search_for_account(
            &owner.token,
            "quasarneedlexyz",
            10,
            &RecallFilter::default(),
        )
        .unwrap();
    assert!(
        hits.iter()
            .all(|hit| hit.memory_id != "imported-cross-scope")
    );
    let hopped = store
        .search_for_account(&owner.token, "anchorplain", 10, &RecallFilter::default())
        .unwrap();
    assert!(
        hopped
            .iter()
            .any(|hit| hit.memory_id == "imported-cross-scope")
    );
    let graph = store
        .graph_for_account(&owner.token, &GraphFilter::default(), None)
        .unwrap();
    assert!(graph.edges.iter().all(|edge| {
        !(edge.source_id == "imported-cross-scope" && edge.target_id == global.memory_id)
    }));
    assert!(graph.edges.iter().any(|edge| {
        edge.source_id == "imported-cross-scope" && edge.target_id == anchor.memory_id
    }));
    fs::remove_file(path).unwrap();
}
