//! Request-side narrowing and the relevance floor for agent search.

use crate::capture::WorkContext;
use crate::memory::{Kind, Memory, Scope};
use crate::process::distill::handoff_id;
use serde::Deserialize;
use std::collections::HashSet;

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
/// matches its own session and repository, branch and worktree; other personal memories match.
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
            if let Some(session) = &filter.session_id
                && memory.id != handoff_id(&memory.owner, session)
            {
                return false;
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

pub fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(str::to_lowercase)
        .collect()
}

/// Absolute evidence floor for agent search and context: a hit must share a technical term,
/// a distinctive title word, or enough distinctive words with the query. Without it a vector
/// channel always returns its nearest neighbours, however unrelated.
pub fn passes_relevance_floor(memory: &Memory, query: &str) -> bool {
    if entity_exact(memory, query) {
        return true;
    }
    let terms = tokenize(query);
    let technical: Vec<&String> = terms.iter().filter(|t| is_technical(t)).collect();
    let distinctive: Vec<&String> = terms.iter().filter(|t| is_distinctive(t)).collect();
    if technical.is_empty() && distinctive.is_empty() {
        return false;
    }
    let title = tokenize(&memory.title);
    let body = tokenize(&format!(
        "{} {} {}",
        memory.summary,
        memory.reason,
        memory.fields.text()
    ));
    let in_title = |token: &str| title.iter().any(|item| item == token);
    let in_text = |token: &str| in_title(token) || body.iter().any(|item| item == token);
    if technical.iter().any(|t| in_text(t)) {
        return true;
    }
    if distinctive.iter().any(|t| in_title(t)) {
        return true;
    }
    if matches!(memory.kind, Kind::Rule | Kind::Decision) && distinctive.iter().any(|t| in_text(t))
    {
        return true;
    }
    // Repeated query words count once. Generated notes and handoffs need two words; one
    // ordinary word, however long, is not enough for them.
    let matched: HashSet<&str> = distinctive
        .iter()
        .filter(|t| in_text(t))
        .map(|t| t.as_str())
        .collect();
    let generated = matches!(memory.kind, Kind::Note | Kind::Handoff);
    let long_word = !generated && distinctive.iter().any(|t| t.len() >= 5 && in_text(t));
    matched.len() >= 2 || long_word
}

fn entity_exact(memory: &Memory, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return false;
    }
    let tokens = tokenize(&query);
    memory.entities.iter().any(|e| {
        let name = e.name.to_lowercase();
        name == query || tokens.contains(&name)
    })
}

fn is_technical(term: &str) -> bool {
    term.contains('_') || term.chars().any(|c| c.is_ascii_digit())
}

fn is_distinctive(term: &str) -> bool {
    const COMMON: &[&str] = &[
        "about", "after", "also", "because", "been", "before", "capture", "could", "files", "from",
        "handoff", "have", "here", "into", "just", "memory", "note", "notes", "only", "over",
        "session", "should", "summary", "test", "tests", "that", "their", "them", "then", "there",
        "they", "this", "tools", "under", "used", "using", "were", "what", "when", "where",
        "which", "while", "will", "with", "work", "would", "your",
    ];
    term.len() >= 4 && !is_technical(term) && !COMMON.contains(&term)
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
    fn handoffs_match_their_session() {
        let mut h = memory("h", Kind::Handoff, Scope::Personal);
        h.id = handoff_id(&h.owner, "s1");
        let for_session = |s: &str| Filter {
            session_id: Some(s.into()),
            ..Filter::default()
        };
        assert!(matches(&h, &for_session("s1")));
        assert!(!matches(&h, &for_session("s2")));
    }

    #[test]
    fn floor_rejects_unrelated_and_accepts_title_words() {
        let mut m = memory("m", Kind::Fact, Scope::Personal);
        m.title = "Billing money is stored as integer cents".into();
        assert!(passes_relevance_floor(&m, "how is billing money stored"));
        assert!(!passes_relevance_floor(&m, "kubernetes ingress"));
        assert!(!passes_relevance_floor(&m, "the and of"));
    }
}
