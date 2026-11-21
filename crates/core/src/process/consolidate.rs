use super::Origin;

/// Cosine at or above this: the newer memory supersedes the older. The cohort (same owner, scope,
/// project and kind) is enforced by `Store::consolidate_memory`'s candidate query.
pub const SUPERSEDE_AT: f32 = 0.92;
/// Cosine in `[RELATE_AT, SUPERSEDE_AT)`: add an automatic `relates` link.
pub const RELATE_AT: f32 = 0.75;
/// Neighbours considered per memory.
pub const CANDIDATES: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Supersede,
    Relate,
}

/// Anything lower, or a non-finite value, is left alone.
pub fn band(cosine: f32) -> Option<Band> {
    if !cosine.is_finite() || cosine > 1.0 + 1e-5 {
        None
    } else if cosine >= SUPERSEDE_AT {
        Some(Band::Supersede)
    } else if cosine >= RELATE_AT {
        Some(Band::Relate)
    } else {
        None
    }
}

/// One side of a near-duplicate pair.
#[derive(Clone, Copy, Debug)]
pub struct Side<'a> {
    pub branch: Option<&'a str>,
    pub origin: Origin,
}

/// The band actually applied (CM-SPEC-008 §4.2). A supersede becomes a plain `relates` when the two
/// memories belong to different branches (branch-specific knowledge), or when an `agent` memory
/// would be replaced by a `system` one.
pub fn guard(band: Band, newer: Side, older: Side) -> Band {
    if band != Band::Supersede {
        return band;
    }
    let other_branch = matches!((newer.branch, older.branch), (Some(a), Some(b)) if a != b);
    let system_over_agent = newer.origin == Origin::System && older.origin == Origin::Agent;
    if other_branch || system_over_agent {
        Band::Relate
    } else {
        Band::Supersede
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_follow_the_thresholds() {
        assert_eq!(band(0.99), Some(Band::Supersede));
        assert_eq!(band(0.92), Some(Band::Supersede));
        assert_eq!(band(0.919), Some(Band::Relate));
        assert_eq!(band(0.75), Some(Band::Relate));
        assert_eq!(band(0.749), None);
        assert_eq!(band(f32::NAN), None);
    }

    #[test]
    fn consolidate_keeps_branch_specific() {
        let on = |branch, origin| Side { branch, origin };
        let main = on(Some("main"), Origin::Agent);
        assert_eq!(guard(Band::Supersede, main, main), Band::Supersede);
        assert_eq!(
            guard(Band::Supersede, main, on(None, Origin::Agent)),
            Band::Supersede
        );
        assert_eq!(
            guard(Band::Supersede, main, on(Some("feat/x"), Origin::Agent)),
            Band::Relate
        );
        // an agent memory is never replaced by a system one; the reverse is fine
        assert_eq!(
            guard(Band::Supersede, on(None, Origin::System), main),
            Band::Relate
        );
        assert_eq!(
            guard(Band::Supersede, main, on(None, Origin::System)),
            Band::Supersede
        );
        assert_eq!(
            guard(Band::Relate, on(None, Origin::System), main),
            Band::Relate
        );
    }
}
