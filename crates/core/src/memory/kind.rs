use super::model::{Kind, Scope};
use crate::error::Error;

/// Memories are not expired. The column stays; every kind is written with `NULL`.
pub fn expires_at(_kind: Kind, _system: bool, _scope: Scope, _created_at: i64) -> Option<i64> {
    None
}

/// The scope a save gets when the agent names none.
pub fn default_scope(kind: Kind, has_project: bool) -> Scope {
    match kind {
        Kind::Decision | Kind::Rule | Kind::Gotcha | Kind::Procedure | Kind::Fact
            if has_project =>
        {
            Scope::Project
        }
        _ => Scope::Personal,
    }
}

/// Applies the kind policy. A preference is always personal; project scope needs a project.
pub fn resolve_scope(
    kind: Kind,
    requested: Option<Scope>,
    has_project: bool,
) -> Result<Scope, Error> {
    let scope = requested.unwrap_or_else(|| default_scope(kind, has_project));
    if kind == Kind::Preference && scope != Scope::Personal {
        return Err(Error::validation("a preference is always personal"));
    }
    if scope == Scope::Project && !has_project {
        return Err(Error::validation(
            "project scope needs an initialized repository",
        ));
    }
    Ok(scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_policy_table() {
        for kind in [
            Kind::Decision,
            Kind::Rule,
            Kind::Gotcha,
            Kind::Procedure,
            Kind::Fact,
        ] {
            assert_eq!(resolve_scope(kind, None, true), Ok(Scope::Project));
            assert_eq!(resolve_scope(kind, None, false), Ok(Scope::Personal));
        }
        assert_eq!(resolve_scope(Kind::Note, None, true), Ok(Scope::Personal));
        assert_eq!(
            resolve_scope(Kind::Preference, None, true),
            Ok(Scope::Personal)
        );
        assert!(resolve_scope(Kind::Preference, Some(Scope::Project), true).is_err());
        assert!(resolve_scope(Kind::Preference, Some(Scope::Global), true).is_err());
        assert!(resolve_scope(Kind::Rule, Some(Scope::Project), false).is_err());
        assert_eq!(
            resolve_scope(Kind::Rule, Some(Scope::Global), false),
            Ok(Scope::Global)
        );
    }

    #[test]
    fn expiry_follows_kind_and_origin() {
        let at = |kind, system, scope| expires_at(kind, system, scope, 10);
        for kind in [
            Kind::Note,
            Kind::Handoff,
            Kind::Decision,
            Kind::Gotcha,
            Kind::Fact,
            Kind::Rule,
        ] {
            assert_eq!(at(kind, true, Scope::Personal), None);
            assert_eq!(at(kind, false, Scope::Project), None);
            assert_eq!(at(kind, true, Scope::Global), None);
        }
    }
}
