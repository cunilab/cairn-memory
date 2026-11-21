//! Request-side narrowing for agent search.

use crate::capture::WorkContext;
use crate::memory::{Kind, Memory, Scope};
use serde::Deserialize;

/// What the caller is working on, and the kind it asks for. With no field set, nothing is
/// narrowed.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Filter {
    #[serde(rename = "sessionId", alias = "session_id", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<WorkContext>,
    #[serde(default)]
    pub kind: Option<Kind>,
}

/// A memory of another kind never matches. Global memories always match. A project memory matches the context's project. A handoff
/// matches the repository, branch and worktree it was written for; other personal memories match.
pub fn matches(memory: &Memory, filter: &Filter) -> bool {
    if filter.kind.is_some_and(|kind| kind != memory.kind) {
        return false;
    }
    if filter.session_id.is_none() && filter.context.is_none() {
        return true;
    }
    match memory.scope {
        Scope::Global => true,
        Scope::Project => filter
            .context
            .as_ref()
            .and_then(|c| c.project_id.as_ref())
            .is_some_and(|id| memory.project_id.as_ref() == Some(id)),
        Scope::Personal => {
            if memory.kind != Kind::Handoff {
                return true;
            }
            filter.context.as_ref().is_none_or(|c| {
                c.repository
                    .as_ref()
                    .is_none_or(|r| memory.context.repo.as_ref() == Some(r))
                    && c.branch
                        .as_ref()
                        .is_none_or(|b| memory.context.branch.as_ref() == Some(b))
                    && c.worktree
                        .as_ref()
                        .is_none_or(|w| memory.context.worktree.as_ref() == Some(w))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::memory;

    #[test]
    fn project_memories_need_the_context_project() {
        let m = memory("a", Kind::Rule, Scope::Project);
        assert!(matches(&m, &Filter::default()));
        let other = Filter {
            context: Some(WorkContext {
                project_id: Some("p2".into()),
                ..WorkContext::default()
            }),
            ..Filter::default()
        };
        assert!(!matches(&m, &other));
        let own = Filter {
            context: Some(WorkContext {
                project_id: Some("p1".into()),
                ..WorkContext::default()
            }),
            ..Filter::default()
        };
        assert!(matches(&m, &own));
    }

    #[test]
    fn kind_filter_narrows_to_that_kind() {
        let m = memory("a", Kind::Rule, Scope::Global);
        let want = |kind| Filter {
            kind: Some(kind),
            ..Filter::default()
        };
        assert!(matches(&m, &want(Kind::Rule)));
        assert!(!matches(&m, &want(Kind::Decision)));
    }

    #[test]
    fn handoffs_match_their_checkout() {
        let mut h = memory("h", Kind::Handoff, Scope::Personal);
        h.context.branch = Some("main".into());
        let on = |b: &str| Filter {
            context: Some(WorkContext {
                branch: Some(b.into()),
                ..WorkContext::default()
            }),
            ..Filter::default()
        };
        assert!(matches(&h, &on("main")));
        assert!(!matches(&h, &on("other")));
    }
}
