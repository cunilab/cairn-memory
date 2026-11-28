use super::*;
use crate::memory::Fields;
use serde_json::json;

fn hit_text(match_: &LiteralMatch) -> String {
    match_
        .segments
        .iter()
        .filter(|segment| segment.hit)
        .map(|segment| segment.text.as_str())
        .collect()
}

#[test]
fn literal_keeps_case_accents_space_and_punctuation() {
    assert!(contains_literal(
        "Retry Worker Budget",
        "retry worker budget"
    ));
    assert!(contains_literal("Café", "café"));
    assert!(contains_literal("e\u{0301}", "é"));
    assert!(!contains_literal("Café", "cafe"));
    assert!(contains_literal("keep  two spaces", "  two"));
    assert!(!contains_literal("keep  two spaces", "keep two"));
    assert!(contains_literal("value is 100%", "%"));
    assert!(contains_literal("cache-ttl", "cache-ttl"));
    assert!(!contains_literal("cache-ttl", "cache ttl"));
    assert!(contains_literal("the cache stays", "the"));
    assert!(!contains_literal(
        "retry worker budget",
        "retry workers budget"
    ));
}

#[test]
fn highlight_marks_the_original_spelling() {
    let parts = memory_parts(
        "Use a ten-minute cache TTL",
        "Rebuild after a short window.",
        &format!(
            "padding {} the phrase retry worker budget occurs only here {}",
            "word ".repeat(20),
            "tail ".repeat(20)
        ),
        "",
        &Fields::parse(crate::memory::Kind::Rule, Some(json!({}))).unwrap(),
    );
    let found = best_match(&parts, "Retry Worker Budget").unwrap();
    assert_eq!(found.field, "reason");
    assert_eq!(found.label, "Reason");
    assert_eq!(hit_text(&found), "retry worker budget");
    assert!(
        found
            .segments
            .iter()
            .any(|segment| segment.text.contains('…'))
    );
}

#[test]
fn field_arrays_do_not_join() {
    let fields = Fields::parse(
        crate::memory::Kind::Procedure,
        Some(json!({"steps": ["alpha", "beta"], "when": "during release"})),
    )
    .unwrap();
    let parts = memory_parts("Procedure title here", "A summary", "A reason", "", &fields);
    assert!(best_match(&parts, "alphabeta").is_none());
    let found = best_match(&parts, "beta").unwrap();
    assert_eq!(found.field, "steps");
    assert_eq!(found.label, "Steps");
    assert_eq!(hit_text(&found), "beta");
}

#[test]
fn exact_title_is_the_whole_title() {
    assert!(title_equals("Café Menu", "café menu"));
    assert!(!title_equals("Café Menu today", "café menu"));
}

#[test]
fn query_limits() {
    assert_eq!(check_query("  budget  ").unwrap(), "budget");
    assert!(check_query("x").is_ok());
    assert!(check_query(&"a".repeat(201)).is_err());
    assert!(check_query("bad\0").is_err());
}
