use crate::capture::WorkContext;
use crate::memory::model::Rank;
use crate::memory::{Kind, Memory, Scope, Status};

/// Total of every adjustment, as a share of the fused score.
pub const ADJUST_CAP: f32 = 0.30;
const FRESH_SECONDS: i64 = 30 * 86_400;

/// The score multiplier is `1 + adjust`: scope closeness, status, freshness, explicit feedback,
/// kind, a lower rank for a candidate, and repeated proof, capped at ±30 %. Reading a memory
/// never raises its score (A17).
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
    let status = match m.status {
        Status::Superseded => -0.30,
        Status::Stale => -0.20,
        _ => 0.0,
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
    let candidate = if m.rank == Rank::Candidate {
        -0.10
    } else {
        0.0
    };
    // 0.02 for each save after the first, and no more than +0.06.
    let proof = 0.02 * (m.proof_count.min(4) as f32 - 1.0);
    (scope + status + fresh + used + kind + candidate + proof).clamp(-ADJUST_CAP, ADJUST_CAP)
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

    #[test]
    fn candidate_ranks_lower() {
        use crate::memory::model::Rank;

        let mut normal = memory("a", Kind::Fact, Scope::Personal);
        normal.updated_at = -100 * 86_400;
        normal.rank = Rank::Normal;
        let mut candidate = normal.clone();
        candidate.rank = Rank::Candidate;
        let plain = WorkContext::default();
        assert_eq!(adjust(&normal, &plain, 0), 0.0);
        assert_eq!(adjust(&candidate, &plain, 0), -0.10);

        let mut low = memory("b", Kind::Handoff, Scope::Global);
        low.status = Status::Stale;
        low.usage.wrong = 9;
        low.updated_at = -100 * 86_400;
        low.rank = Rank::Candidate;
        assert_eq!(adjust(&low, &ctx(), 0), -ADJUST_CAP);
    }

    #[test]
    fn superseded_sits_at_the_floor() {
        use crate::memory::model::Rank;

        let mut m = memory("a", Kind::Fact, Scope::Personal);
        m.updated_at = -100 * 86_400;
        m.status = Status::Superseded;
        let plain = WorkContext::default();
        assert_eq!(adjust(&m, &plain, 0), -ADJUST_CAP);
        m.rank = Rank::Candidate;
        assert_eq!(adjust(&m, &plain, 0), -ADJUST_CAP);
    }

    #[test]
    fn proof_count_boost_capped() {
        let mut plain_memory = memory("a", Kind::Fact, Scope::Personal);
        plain_memory.updated_at = -100 * 86_400;
        let plain = WorkContext::default();
        assert_eq!(adjust(&plain_memory, &plain, 0), 0.0);
        plain_memory.proof_count = 2;
        assert_eq!(adjust(&plain_memory, &plain, 0), 0.02);
        plain_memory.proof_count = 3;
        assert_eq!(adjust(&plain_memory, &plain, 0), 0.04);
        plain_memory.proof_count = 4;
        assert_eq!(adjust(&plain_memory, &plain, 0), 0.06);
        plain_memory.proof_count = 9;
        assert_eq!(adjust(&plain_memory, &plain, 0), 0.06);

        let mut best = memory("b", Kind::Rule, Scope::Project);
        best.context.branch = Some("main".into());
        best.usage.helpful = 9;
        best.proof_count = 9;
        assert_eq!(adjust(&best, &ctx(), 0), ADJUST_CAP);

        let mut low = memory("c", Kind::Handoff, Scope::Global);
        low.status = Status::Stale;
        low.usage.wrong = 9;
        low.updated_at = -100 * 86_400;
        low.proof_count = 9;
        assert_eq!(adjust(&low, &ctx(), 0), -ADJUST_CAP);
    }
}
