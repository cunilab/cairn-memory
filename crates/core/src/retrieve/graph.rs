//! The memory graph: nodes are memories, edges are `links` rows. The JSON keys match what the
//! web console reads today (`memory_id`, `source_id`, `target_id`); step 7 renames them.

use crate::error::Error;
use crate::memory::{Kind, Link, Memory, Relation, Scope};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

pub const NODE_CAP_DEFAULT: usize = 120;
pub const NODE_CAP_MIN: usize = 120;
pub const NODE_CAP_MAX: usize = 500;

#[derive(Clone, Debug, Default)]
pub struct GraphFilter {
    pub scope: Option<Scope>,
    pub project_id: Option<String>,
    pub relation: Option<Relation>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Node {
    pub memory_id: String,
    pub title: String,
    pub scope: Scope,
    pub project_id: Option<String>,
    pub kind: Kind,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Edge {
    pub source_id: String,
    pub target_id: String,
    pub relation: Relation,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub truncated: bool,
    pub node_cap: usize,
}

pub fn node_cap(requested: Option<usize>) -> Result<usize, Error> {
    match requested {
        None => Ok(NODE_CAP_DEFAULT),
        Some(cap) if (NODE_CAP_MIN..=NODE_CAP_MAX).contains(&cap) => Ok(cap),
        Some(_) => Err(Error::validation("graph cap must be from 120 through 500")),
    }
}

/// `memories` are already visible to the caller; `links` are the rows between them. An edge
/// stays only when both ends are candidate nodes. Above `cap`, the fewest incident edges go
/// first, then the oldest, then the id.
pub fn build(memories: &[Memory], links: &[Link], filter: &GraphFilter, cap: usize) -> Graph {
    let mut candidates: Vec<&Memory> = memories
        .iter()
        .filter(|m| filter.scope.is_none_or(|s| m.scope == s))
        .filter(|m| {
            filter
                .project_id
                .as_ref()
                .is_none_or(|p| m.project_id.as_ref() == Some(p))
        })
        .collect();
    let ids: HashSet<&str> = candidates.iter().map(|m| m.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut edges: Vec<Edge> = links
        .iter()
        .filter(|l| l.source != l.target)
        .filter(|l| ids.contains(l.source.as_str()) && ids.contains(l.target.as_str()))
        .filter(|l| filter.relation.is_none_or(|r| r == l.relation))
        .filter(|l| seen.insert((l.source.clone(), l.relation.as_str(), l.target.clone())))
        .map(|l| Edge {
            source_id: l.source.clone(),
            target_id: l.target.clone(),
            relation: l.relation,
        })
        .collect();
    if filter.relation.is_some() {
        let touched: HashSet<&str> = edges
            .iter()
            .flat_map(|e| [e.source_id.as_str(), e.target_id.as_str()])
            .collect();
        candidates.retain(|m| touched.contains(m.id.as_str()));
    }
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for edge in &edges {
        *degree.entry(edge.source_id.as_str()).or_default() += 1;
        *degree.entry(edge.target_id.as_str()).or_default() += 1;
    }
    let truncated = candidates.len() > cap;
    if truncated {
        candidates.sort_by(|a, b| {
            let d = |m: &Memory| degree.get(m.id.as_str()).copied().unwrap_or(0);
            d(a).cmp(&d(b))
                .then(a.created_us.cmp(&b.created_us))
                .then(a.id.cmp(&b.id))
        });
        let removed: HashSet<String> = candidates
            .drain(..candidates.len() - cap)
            .map(|m| m.id.clone())
            .collect();
        edges.retain(|e| !removed.contains(&e.source_id) && !removed.contains(&e.target_id));
    }
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    edges.sort_by(|a, b| {
        a.source_id
            .cmp(&b.source_id)
            .then(a.relation.as_str().cmp(b.relation.as_str()))
            .then(a.target_id.cmp(&b.target_id))
    });
    Graph {
        nodes: candidates
            .into_iter()
            .map(|m| Node {
                memory_id: m.id.clone(),
                title: m.title.clone(),
                scope: m.scope,
                project_id: m.project_id.clone(),
                kind: m.kind,
            })
            .collect(),
        edges,
        truncated,
        node_cap: cap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::memory;

    fn link(source: &str, target: &str, relation: Relation) -> Link {
        Link {
            relation,
            source: source.into(),
            target: target.into(),
            auto: false,
        }
    }

    #[test]
    fn edges_need_both_ends_and_filters_prune_nodes() {
        let a = memory("a", Kind::Rule, Scope::Personal);
        let b = memory("b", Kind::Fact, Scope::Personal);
        let c = memory("c", Kind::Fact, Scope::Personal);
        let links = [
            link("a", "b", Relation::Supersedes),
            link("a", "b", Relation::Supersedes),
            link("a", "gone", Relation::Relates),
        ];
        let graph = build(
            &[a.clone(), b.clone(), c.clone()],
            &links,
            &GraphFilter::default(),
            120,
        );
        assert_eq!((graph.nodes.len(), graph.edges.len()), (3, 1));
        let only = GraphFilter {
            relation: Some(Relation::Supersedes),
            ..GraphFilter::default()
        };
        let graph = build(&[a, b, c], &links, &only, 120);
        assert_eq!(graph.nodes.len(), 2);
        assert!(!graph.truncated);
    }

    #[test]
    fn cap_range_is_enforced() {
        assert_eq!(node_cap(None), Ok(120));
        assert!(node_cap(Some(10)).is_err());
        assert_eq!(node_cap(Some(500)), Ok(500));
    }
}
