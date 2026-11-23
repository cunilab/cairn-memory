use super::*;

#[test]
fn f1_threshold() {
    assert!(f1("project", 3, 0, true).is_some());
    assert!(f1("project", 2, 0, true).is_none());
    assert!(f1("project", 3, 1, true).is_none());
    assert!(f1("project", 3, 0, false).is_none());
}

#[test]
fn f2_threshold() {
    assert!(f2("memory", 7, true).is_some());
    assert!(f2("memory", 6, true).is_none());
    assert!(f2("memory", 7, false).is_none());
}

#[test]
fn f3_threshold() {
    assert!(f3("memory", 7, true).is_some());
    assert!(f3("memory", 6, true).is_none());
    assert!(f3("memory", 7, false).is_none());
}

#[test]
fn f4_threshold() {
    assert!(f4("device", 8, 1, true).is_some());
    assert!(f4("device", 7, 1, true).is_none());
    assert!(f4("device", 8, 2, true).is_none());
    assert!(f4("device", 8, 1, false).is_none());
}

#[test]
fn f5_threshold() {
    assert!(f5("device", "0.1.8", "0.1.9").is_some());
    assert!(f5("device", "0.1.9", "0.1.9").is_none());
    assert!(f5("device", "invalid", "0.1.9").is_none());
}

#[test]
fn f6_threshold() {
    assert!(
        f6(
            "device",
            2,
            &["project".into()],
            Some(("project", "repo", "name"))
        )
        .is_some()
    );
    assert!(
        f6(
            "device",
            1,
            &["project".into()],
            Some(("project", "repo", "name"))
        )
        .is_none()
    );
    assert!(f6("device", 2, &[], None).is_none());
}

#[test]
fn f7_threshold() {
    assert!(f7("source", "target", "node 20", "node 22").is_some());
    assert!(f7("source", "target", "node 20", "node 20").is_none());
}

#[test]
fn f8_threshold() {
    assert!(f8(false, 0, 0).is_some());
    assert!(f8(true, 1, 0).is_some());
    assert!(f8(true, 0, 1).is_some());
    assert!(f8(true, 0, 0).is_none());
}

#[test]
fn f9_threshold() {
    assert!(f9("memory", "title", "token=example-secret").is_some());
    assert!(f9("memory", "title", "ordinary title").is_none());
}

#[test]
fn f10_threshold() {
    assert!(f10("project", 14, 10, 1).is_some());
    assert!(f10("project", 13, 10, 1).is_none());
    assert!(f10("project", 14, 9, 1).is_none());
    assert!(f10("project", 14, 10, 2).is_none());
}

#[test]
fn f11_threshold() {
    assert!(f11(("memory", "phrase"), 14, 2, 0.43, 4, true).is_some());
    assert!(f11(("memory", "phrase"), 13, 2, 0.43, 4, true).is_none());
    assert!(f11(("memory", "phrase"), 14, 1, 0.43, 4, true).is_none());
    assert!(f11(("memory", "phrase"), 14, 2, 0.42, 4, true).is_none());
    assert!(f11(("memory", "phrase"), 14, 2, 0.43, 5, true).is_none());
    assert!(f11(("memory", "phrase"), 14, 2, 0.43, 4, false).is_none());
}

#[test]
fn f12_threshold() {
    assert!(f12("memory", 91, 90, 20, true).is_some());
    assert!(f12("memory", 90, 90, 20, true).is_none());
    assert!(f12("memory", 91, 89, 20, true).is_none());
    assert!(f12("memory", 91, 90, 19, true).is_none());
    assert!(f12("memory", 91, 90, 20, false).is_none());
}
