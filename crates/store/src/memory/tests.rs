use super::*;

#[test]
fn pin_limits_follow_scope() {
    assert_eq!(pin_limit(Scope::Personal), Some(5));
    assert_eq!(pin_limit(Scope::Project), Some(10));
    assert_eq!(pin_limit(Scope::Global), None);
}
