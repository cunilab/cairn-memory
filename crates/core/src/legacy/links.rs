use crate::legacy::lifecycle::{MemoryKind, MemoryScope};
use crate::legacy::memories::Memory;
use crate::legacy::store::MemoryStore;
use crate::legacy::types::{StoreError, now_seconds};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const GRAPH_NODE_CAP_DEFAULT: usize = 120;
pub const GRAPH_NODE_CAP_MIN: usize = 120;
pub const GRAPH_NODE_CAP_MAX: usize = 500;

/// Directed relations a caller can create. `contradicts` and `supersedes` stay
/// on their own fields.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LinkRelation {
    Relates,
    Causes,
    Fixes,
    DependsOn,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryLink {
    pub relation: LinkRelation,
    pub target_id: String,
}

/// Why an imported link was not stored. Link creation rejects the same cases.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DroppedLinkReason {
    SelfLink,
    Duplicate,
    UnknownTarget,
    Scope,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DroppedLink {
    pub memory_id: String,
    pub relation: LinkRelation,
    pub target_id: String,
    pub reason: DroppedLinkReason,
}

/// A stored directed link counts only when both ends are different memories in
/// the same scope and project. Missing and cross-scope targets do not.
pub fn link_is_readable(source: &Memory, target: &Memory) -> bool {
    source.memory_id != target.memory_id
        && source.scope == target.scope
        && source.project_id == target.project_id
}

/// Every edge the canvas can draw, including the two stored outside `links`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GraphRelation {
    Relates,
    Causes,
    Fixes,
    DependsOn,
    Contradicts,
    Supersedes,
}

impl From<LinkRelation> for GraphRelation {
    fn from(relation: LinkRelation) -> Self {
        match relation {
            LinkRelation::Relates => Self::Relates,
            LinkRelation::Causes => Self::Causes,
            LinkRelation::Fixes => Self::Fixes,
            LinkRelation::DependsOn => Self::DependsOn,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphFilter {
    pub scope: Option<MemoryScope>,
    pub project_id: Option<String>,
    pub relation: Option<GraphRelation>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct GraphNode {
    pub memory_id: String,
    pub title: String,
    pub scope: MemoryScope,
    pub project_id: Option<String>,
    pub kind: MemoryKind,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub source_id: String,
    pub target_id: String,
    pub relation: GraphRelation,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MemoryGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated: bool,
    pub node_cap: usize,
}

pub fn graph_node_cap(requested: Option<usize>) -> Result<usize, StoreError> {
    match requested {
        None => Ok(GRAPH_NODE_CAP_DEFAULT),
        Some(cap) if (GRAPH_NODE_CAP_MIN..=GRAPH_NODE_CAP_MAX).contains(&cap) => Ok(cap),
        Some(_) => Err(StoreError::Validation(
            "graph cap must be from 120 through 500".to_owned(),
        )),
    }
}

/// Nodes are the memories the caller can already read. An edge is kept only
/// when both ends are in that set and share a scope and project. Above `cap`, the fewest incident edges are
/// dropped first, then the oldest `created_at`, then the memory id.
pub fn build_memory_graph(memories: &[Memory], filter: &GraphFilter, cap: usize) -> MemoryGraph {
    let mut candidates: Vec<&Memory> = memories
        .iter()
        .filter(|memory| {
            filter
                .scope
                .as_ref()
                .is_none_or(|scope| &memory.scope == scope)
                && filter
                    .project_id
                    .as_ref()
                    .is_none_or(|project_id| memory.project_id.as_ref() == Some(project_id))
        })
        .collect();
    let by_id = candidates
        .iter()
        .map(|memory| (memory.memory_id.as_str(), *memory))
        .collect::<HashMap<_, _>>();
    let ids = by_id.keys().copied().collect::<HashSet<_>>();
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for memory in &candidates {
        for link in &memory.links {
            let Some(target) = by_id.get(link.target_id.as_str()) else {
                continue;
            };
            if !link_is_readable(memory, target) {
                continue;
            }
            push_edge(
                &mut edges,
                &mut seen,
                &ids,
                filter.relation,
                &memory.memory_id,
                &link.target_id,
                link.relation.into(),
            );
        }
        for target in &memory.contradicts {
            push_edge(
                &mut edges,
                &mut seen,
                &ids,
                filter.relation,
                &memory.memory_id,
                target,
                GraphRelation::Contradicts,
            );
        }
        if let Some(target) = &memory.supersedes {
            push_edge(
                &mut edges,
                &mut seen,
                &ids,
                filter.relation,
                &memory.memory_id,
                target,
                GraphRelation::Supersedes,
            );
        }
    }
    if filter.relation.is_some() {
        let touched = edges
            .iter()
            .flat_map(|edge| [edge.source_id.as_str(), edge.target_id.as_str()])
            .collect::<HashSet<_>>();
        candidates.retain(|memory| touched.contains(memory.memory_id.as_str()));
    }
    let mut degree = HashMap::<&str, usize>::new();
    for edge in &edges {
        *degree.entry(edge.source_id.as_str()).or_default() += 1;
        *degree.entry(edge.target_id.as_str()).or_default() += 1;
    }
    let truncated = candidates.len() > cap;
    if truncated {
        candidates.sort_by(|left, right| {
            degree
                .get(left.memory_id.as_str())
                .copied()
                .unwrap_or(0)
                .cmp(&degree.get(right.memory_id.as_str()).copied().unwrap_or(0))
                .then_with(|| {
                    created_stamp(&left.created_at).cmp(&created_stamp(&right.created_at))
                })
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        let removed = candidates
            .drain(..candidates.len() - cap)
            .map(|memory| memory.memory_id.clone())
            .collect::<HashSet<_>>();
        edges.retain(|edge| {
            !removed.contains(&edge.source_id) && !removed.contains(&edge.target_id)
        });
    }
    candidates.sort_by(|left, right| left.memory_id.cmp(&right.memory_id));
    edges.sort_by(|left, right| {
        left.source_id
            .cmp(&right.source_id)
            .then(left.relation.cmp(&right.relation))
            .then(left.target_id.cmp(&right.target_id))
    });
    MemoryGraph {
        nodes: candidates
            .into_iter()
            .map(|memory| GraphNode {
                memory_id: memory.memory_id.clone(),
                title: memory.fields.title.clone(),
                scope: memory.scope.clone(),
                project_id: memory.project_id.clone(),
                kind: memory.kind,
            })
            .collect(),
        edges,
        truncated,
        node_cap: cap,
    }
}

fn push_edge(
    edges: &mut Vec<GraphEdge>,
    seen: &mut HashSet<(String, GraphRelation, String)>,
    ids: &HashSet<&str>,
    wanted: Option<GraphRelation>,
    source_id: &str,
    target_id: &str,
    relation: GraphRelation,
) {
    if source_id == target_id || !ids.contains(target_id) {
        return;
    }
    if wanted.is_some_and(|wanted| wanted != relation) {
        return;
    }
    if !seen.insert((source_id.to_owned(), relation, target_id.to_owned())) {
        return;
    }
    edges.push(GraphEdge {
        source_id: source_id.to_owned(),
        target_id: target_id.to_owned(),
        relation,
    });
}

fn created_stamp(value: &str) -> i64 {
    value.parse().unwrap_or(i64::MAX)
}

fn same_link_scope(left: &Memory, right: &Memory) -> Result<(), StoreError> {
    if left.scope == right.scope && left.project_id == right.project_id {
        Ok(())
    } else {
        Err(StoreError::Validation(
            "link must stay in the same scope and project".to_owned(),
        ))
    }
}

struct LinkEnd {
    scope: crate::legacy::MemoryScope,
    project_id: Option<String>,
}

/// Drops imported links that `link_memories` would reject. A target may be
/// another row in this bundle or a memory the caller can already read.
pub fn sanitize_imported_links(incoming: &mut [Memory], readable: &[Memory]) -> Vec<DroppedLink> {
    let mut known = HashMap::<String, LinkEnd>::new();
    for memory in readable {
        known.insert(
            memory.memory_id.clone(),
            LinkEnd {
                scope: memory.scope.clone(),
                project_id: memory.project_id.clone(),
            },
        );
    }
    for memory in incoming.iter() {
        known.insert(
            memory.memory_id.clone(),
            LinkEnd {
                scope: memory.scope.clone(),
                project_id: memory.project_id.clone(),
            },
        );
    }
    let mut dropped = Vec::new();
    for memory in incoming.iter_mut() {
        let mut kept = Vec::new();
        let mut seen = HashSet::new();
        for link in std::mem::take(&mut memory.links) {
            if link.target_id == memory.memory_id {
                dropped.push(dropped_link(memory, &link, DroppedLinkReason::SelfLink));
                continue;
            }
            if !seen.insert((link.relation, link.target_id.clone())) {
                dropped.push(dropped_link(memory, &link, DroppedLinkReason::Duplicate));
                continue;
            }
            let Some(target) = known.get(&link.target_id) else {
                dropped.push(dropped_link(
                    memory,
                    &link,
                    DroppedLinkReason::UnknownTarget,
                ));
                continue;
            };
            if memory.scope != target.scope || memory.project_id != target.project_id {
                dropped.push(dropped_link(memory, &link, DroppedLinkReason::Scope));
                continue;
            }
            kept.push(link);
        }
        memory.links = kept;
    }
    dropped
}

fn dropped_link(memory: &Memory, link: &MemoryLink, reason: DroppedLinkReason) -> DroppedLink {
    DroppedLink {
        memory_id: memory.memory_id.clone(),
        relation: link.relation,
        target_id: link.target_id.clone(),
        reason,
    }
}

impl MemoryStore {
    fn readable_memory(&self, token: &str, memory_id: &str) -> Result<Memory, StoreError> {
        self.memories_for_account(token)?
            .into_iter()
            .find(|memory| memory.memory_id == memory_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))
    }

    pub fn link_memories(
        &mut self,
        token: &str,
        source_id: &str,
        relation: LinkRelation,
        target_id: &str,
    ) -> Result<Memory, StoreError> {
        self.write_link(token, source_id, relation, target_id, true)
    }

    pub fn unlink_memories(
        &mut self,
        token: &str,
        source_id: &str,
        relation: LinkRelation,
        target_id: &str,
    ) -> Result<Memory, StoreError> {
        self.write_link(token, source_id, relation, target_id, false)
    }

    fn write_link(
        &mut self,
        token: &str,
        source_id: &str,
        relation: LinkRelation,
        target_id: &str,
        add: bool,
    ) -> Result<Memory, StoreError> {
        if source_id == target_id {
            return Err(StoreError::Validation(
                "a memory cannot link to itself".to_owned(),
            ));
        }
        let source = self.readable_memory(token, source_id)?;
        let target = self.readable_memory(token, target_id)?;
        same_link_scope(&source, &target)?;
        let account = self.authenticate_account(token)?;
        self.reject_member_mutation(&account, source_id)?;
        let link = MemoryLink {
            relation,
            target_id: target_id.to_owned(),
        };
        let memory = self
            .state
            .memories
            .iter_mut()
            .find(|memory| memory.memory_id == source_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {source_id}")))?;
        let present = memory.links.iter().any(|existing| existing == &link);
        if add && !present {
            memory.links.push(link);
            memory.changed_by = Some(account.person_id.clone());
            memory.changed_at = Some(now_seconds().to_string());
        } else if !add && present {
            memory.links.retain(|existing| existing != &link);
            memory.changed_by = Some(account.person_id.clone());
            memory.changed_at = Some(now_seconds().to_string());
        }
        let stored = memory.clone();
        self.persist()?;
        Ok(stored)
    }

    pub fn graph_for_account(
        &self,
        token: &str,
        filter: &GraphFilter,
        cap: Option<usize>,
    ) -> Result<MemoryGraph, StoreError> {
        let cap = graph_node_cap(cap)?;
        let memories = self.memories_for_account(token)?;
        Ok(build_memory_graph(&memories, filter, cap))
    }
}

#[cfg(test)]
mod tests;
