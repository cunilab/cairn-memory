use crate::memory::Memory;

/// Cosine at or above this: the newer memory supersedes the older (same cohort only).
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

/// Auto-supersede needs the same owner, scope, project and kind. Other pairs may only relate.
pub fn same_cohort(a: &Memory, b: &Memory) -> bool {
    a.owner == b.owner && a.scope == b.scope && a.project_id == b.project_id && a.kind == b.kind
}

/// The band to apply to a pair: a supersede band outside one cohort degrades to nothing,
/// because pairs from other cohorts are never candidates (fixes A2).
pub fn band_for(a: &Memory, b: &Memory, cosine: f32) -> Option<Band> {
    if same_cohort(a, b) {
        band(cosine)
    } else {
        None
    }
}

/// Which of two memories is the newer one: later `created_at`, ties broken by id.
pub fn newer<'a>(a: &'a Memory, b: &'a Memory) -> &'a Memory {
    if (a.created_at, &a.id) >= (b.created_at, &b.id) {
        a
    } else {
        b
    }
}

pub fn cosine(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let (mut dot, mut ln, mut rn) = (0.0_f32, 0.0_f32, 0.0_f32);
    for (a, b) in left.iter().zip(right) {
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        dot += a * b;
        ln += a * a;
        rn += b * b;
    }
    let denom = ln.sqrt() * rn.sqrt();
    (denom > 0.0).then(|| (dot / denom).clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Kind, Scope};
    use crate::testing::memory;

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
    fn supersede_needs_one_cohort() {
        let a = memory("a", Kind::Rule, Scope::Project);
        let mut b = memory("b", Kind::Rule, Scope::Project);
        assert_eq!(band_for(&a, &b, 0.95), Some(Band::Supersede));
        b.kind = Kind::Fact;
        assert_eq!(band_for(&a, &b, 0.95), None);
        let mut c = memory("c", Kind::Rule, Scope::Project);
        c.project_id = Some("other".into());
        assert_eq!(band_for(&a, &c, 0.95), None);
        c.project_id = a.project_id.clone();
        c.owner = "someone".into();
        assert_eq!(band_for(&a, &c, 0.95), None);
    }

    #[test]
    fn cosine_of_unit_vectors() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), Some(0.0));
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), None);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), None);
    }

    #[test]
    fn newer_uses_created_then_id() {
        let mut a = memory("a", Kind::Rule, Scope::Project);
        let mut b = memory("b", Kind::Rule, Scope::Project);
        a.created_at = 5;
        b.created_at = 5;
        assert_eq!(newer(&a, &b).id, "b");
        a.created_at = 9;
        assert_eq!(newer(&a, &b).id, "a");
    }
}
