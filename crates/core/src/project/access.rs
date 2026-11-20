use crate::memory::{Memory, Scope, Status};

/// The caller of a read: who they are and which projects they belong to.
pub struct Viewer<'a> {
    pub person_id: &'a str,
    pub project_ids: &'a [String],
}

/// The Rust twin of the store's visibility SQL fragment:
/// not forgotten, not expired, and owned, global, or in one of the viewer's projects.
pub fn is_visible(memory: &Memory, viewer: &Viewer<'_>, now: i64) -> bool {
    memory.status != Status::Forgotten
        && memory.expires_at.is_none_or(|expires| expires > now)
        && (memory.owner == viewer.person_id
            || memory.scope == Scope::Global
            || (memory.scope == Scope::Project
                && memory
                    .project_id
                    .as_ref()
                    .is_some_and(|id| viewer.project_ids.contains(id))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Kind;
    use crate::testing::memory;

    #[test]
    fn visibility_matrix() {
        let projects = vec!["p1".to_owned()];
        let me = Viewer {
            person_id: "me",
            project_ids: &projects,
        };
        let stranger = Viewer {
            person_id: "x",
            project_ids: &[],
        };
        let mut m = memory("m", Kind::Rule, Scope::Project);
        m.owner = "me".into();
        m.project_id = Some("p1".into());
        assert!(is_visible(&m, &me, 0));
        assert!(!is_visible(&m, &stranger, 0));
        let member = Viewer {
            person_id: "y",
            project_ids: &projects,
        };
        assert!(is_visible(&m, &member, 0));
        m.scope = Scope::Personal;
        assert!(!is_visible(&m, &member, 0));
        m.scope = Scope::Global;
        assert!(is_visible(&m, &stranger, 0));
        m.expires_at = Some(10);
        assert!(is_visible(&m, &stranger, 9) && !is_visible(&m, &stranger, 10));
        m.expires_at = None;
        m.status = Status::Forgotten;
        assert!(!is_visible(&m, &me, 0));
    }
}
