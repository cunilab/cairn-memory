use super::model::{Kind, Scope};
use crate::error::Error;

pub const NOTE_TTL_SECONDS: i64 = 90 * 86_400;
pub const HANDOFF_TTL_SECONDS: i64 = 14 * 86_400;
/// Expired memories are hard-deleted by the idle sweep this long after expiry.
pub const EXPIRED_GRACE_SECONDS: i64 = 30 * 86_400;

pub fn expires_at(kind: Kind, created_at: i64) -> Option<i64> {
    match kind {
        Kind::Note => Some(created_at + NOTE_TTL_SECONDS),
        Kind::Handoff => Some(created_at + HANDOFF_TTL_SECONDS),
        _ => None,
    }
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
    fn only_note_and_handoff_expire() {
        assert_eq!(expires_at(Kind::Note, 10), Some(10 + 90 * 86_400));
        assert_eq!(expires_at(Kind::Handoff, 10), Some(10 + 14 * 86_400));
        assert_eq!(expires_at(Kind::Decision, 10), None);
    }
}
