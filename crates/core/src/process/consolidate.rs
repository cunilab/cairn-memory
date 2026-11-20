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
}
