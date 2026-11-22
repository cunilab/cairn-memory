use crate::capture::WorkContext;
use crate::memory::{Kind, Memory, Scope, Status};

/// Total of every adjustment, as a share of the fused score.
pub const ADJUST_CAP: f32 = 0.30;
const FRESH_SECONDS: i64 = 30 * 86_400;

/// The score multiplier is `1 + adjust`: scope closeness, status, freshness, explicit feedback and
/// kind, capped at ±30 %. Reading a memory never raises its score (A17).
pub fn adjust(m: &Memory, ctx: &WorkContext, now: i64) -> f32 {
    let scope = match m.scope {
        _ if ctx.project_id.is_some()
            && ctx.branch.is_some()
            && m.context.branch == ctx.branch
            && m.project_id == ctx.project_id =>
        {
            0.10
        }
        Scope::Project if ctx.project_id.is_some() && m.project_id == ctx.project_id => 0.05,
        Scope::Global => -0.05,
        _ => 0.0,
    };
    let status = if m.status == Status::Stale {
        -0.20
    } else {
        0.0
    };
    let fresh = if now - m.updated_at <= FRESH_SECONDS {
        0.05
    } else {
        0.0
    };
    let net = m.usage.helpful as f32 - m.usage.wrong as f32;
    let used = (0.05 * net).clamp(-0.10, 0.10);
    let kind = match m.kind {
        Kind::Rule | Kind::Decision => 0.10,
        Kind::Gotcha | Kind::Procedure => 0.05,
        Kind::Fact | Kind::Preference => 0.0,
        Kind::Note => -0.05,
        Kind::Handoff => -0.10,
    };
    (scope + status + fresh + used + kind).clamp(-ADJUST_CAP, ADJUST_CAP)
}

/// The status after the votes changed. `wrong >= 2`, or `wrong >= 1` with the owner among them,
/// makes an `active` memory `stale`; a memory that went stale by feedback (`by_feedback`) is
/// `active` again once that no longer holds, because a voter changed their vote. A memory that is
/// stale for a missing file, or any other status, never changes here.
pub fn status_after_votes(
    current: Status,
    by_feedback: bool,
    wrong: u32,
    owner_wrong: bool,
) -> Status {
    let stale = wrong >= 2 || (wrong >= 1 && owner_wrong);
    match current {
        Status::Active if stale => Status::Stale,
        Status::Stale if by_feedback && !stale => Status::Active,
        _ => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::memory;

    fn ctx() -> WorkContext {
        WorkContext {
            project_id: Some("p1".into()),
            branch: Some("main".into()),
            ..WorkContext::default()
        }
    }

    #[test]
    fn adjust_is_capped() {
        let mut best = memory("a", Kind::Rule, Scope::Project);
        best.context.branch = Some("main".into());
        best.usage.helpful = 9;
        assert_eq!(adjust(&best, &ctx(), 0), ADJUST_CAP);
        let mut worst = memory("b", Kind::Handoff, Scope::Global);
        worst.status = Status::Stale;
        worst.usage.wrong = 9;
        worst.updated_at = -100 * 86_400;
        assert_eq!(adjust(&worst, &ctx(), 0), -ADJUST_CAP);
    }

    #[test]
    fn branch_closeness_needs_a_project_context() {
        // Without a project, another repo's "main" must not look close.
        let mut m = memory("a", Kind::Fact, Scope::Personal);
        m.context.branch = Some("main".into());
        m.updated_at = -100 * 86_400;
        let no_project = WorkContext {
            branch: Some("main".into()),
            ..WorkContext::default()
        };
        assert_eq!(adjust(&m, &no_project, 0), 0.0);
    }

    #[test]
    fn reading_never_raises_the_score() {
        let mut m = memory("a", Kind::Fact, Scope::Personal);
        m.updated_at = -100 * 86_400;
        let before = adjust(&m, &ctx(), 0);
        m.usage.fetched = 99;
        assert_eq!(adjust(&m, &ctx(), 0), before);
        assert_eq!(before, 0.0);
    }

    #[test]
    fn staleness_rule() {
        let after = |current, by_feedback, wrong, owner| {
            status_after_votes(current, by_feedback, wrong, owner)
        };
        assert_eq!(after(Status::Active, false, 1, false), Status::Active);
        assert_eq!(after(Status::Active, false, 1, true), Status::Stale);
        assert_eq!(after(Status::Active, false, 2, false), Status::Stale);
        assert_eq!(
            after(Status::Superseded, false, 5, true),
            Status::Superseded
        );
        // The voter who made it stale changes their vote: back to active.
        assert_eq!(after(Status::Stale, true, 0, false), Status::Active);
        assert_eq!(after(Status::Stale, true, 1, false), Status::Active);
        assert_eq!(after(Status::Stale, true, 1, true), Status::Stale);
        // A memory stale for a missing file is not revived by votes.
        assert_eq!(after(Status::Stale, false, 0, false), Status::Stale);
    }
}
