use super::*;

#[test]
fn skips_short_acknowledgements_and_slash_commands() {
    for prompt in [
        "",
        "with sol",
        "Do it now",
        "  /run these tests please",
        "yes, okay, go ahead",
        "please do that",
        "please go on and continue",
    ] {
        assert!(!should_recall(prompt), "{prompt}");
    }
    assert!(should_recall("Please proceed with sol"));
    assert!(should_recall("Explain why tests fail"));
}

#[test]
fn a_named_preference_requires_the_same_cue() {
    assert_eq!(
        named_preference("Please run this with sol", "Sol worker", "Use it with Sol"),
        Some("sol")
    );
    assert_eq!(
        named_preference("Please run this with sol", "Solar worker", "Run with solar"),
        None
    );
    assert_eq!(
        named_preference("Please run this with sol", "Sol worker", "Use Sol"),
        Some("sol")
    );
}
