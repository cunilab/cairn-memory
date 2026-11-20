use crate::memory::{Status, Usage};

pub const FEEDBACK_STEP: f32 = 0.15;
pub const FEEDBACK_CAP: f32 = 0.45;

/// Ranking multiplier from explicit feedback only. Reading a memory never raises its score (A17).
pub fn multiplier(usage: &Usage) -> f32 {
    let net = usage.helpful as f32 - usage.wrong as f32;
    (1.0 + FEEDBACK_STEP * net).clamp(1.0 - FEEDBACK_CAP, 1.0 + FEEDBACK_CAP)
}

/// `stale` when `wrong >= 2`, or `wrong >= 1` from the owner. Other statuses never change here.
pub fn status_after_wrong(current: Status, wrong: u32, owner_voted: bool) -> Status {
    if current == Status::Active && (wrong >= 2 || (wrong >= 1 && owner_voted)) {
        Status::Stale
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_moves_the_multiplier_within_the_cap() {
        let usage = |helpful, wrong| Usage {
            helpful,
            wrong,
            ..Usage::default()
        };
        assert_eq!(multiplier(&usage(0, 0)), 1.0);
        assert!(multiplier(&usage(2, 0)) > 1.0);
        assert!((multiplier(&usage(0, 1)) - 0.85).abs() < 1e-6);
        assert!((multiplier(&usage(0, 9)) - 0.55).abs() < 1e-6);
        assert!((multiplier(&usage(9, 0)) - 1.45).abs() < 1e-6);
        let read_often = Usage {
            fetched: 99,
            ..Usage::default()
        };
        assert_eq!(multiplier(&read_often), 1.0);
    }

    #[test]
    fn staleness_rule() {
        assert_eq!(status_after_wrong(Status::Active, 1, false), Status::Active);
        assert_eq!(status_after_wrong(Status::Active, 1, true), Status::Stale);
        assert_eq!(status_after_wrong(Status::Active, 2, false), Status::Stale);
        assert_eq!(
            status_after_wrong(Status::Superseded, 5, true),
            Status::Superseded
        );
    }
}
