use super::should_mute;

#[test]
fn mute_only_three_or_more_never_used_pushes() {
    assert!(!should_mute(2, 0, false));
    assert!(should_mute(3, 0, false));
    assert!(should_mute(4, 0, false));
    assert!(!should_mute(4, 1, false));
    assert!(!should_mute(4, 0, true));
}
