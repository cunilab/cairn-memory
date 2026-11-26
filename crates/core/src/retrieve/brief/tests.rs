use super::*;
use crate::memory::Scope;
use crate::testing::memory;
use serde_json::Value;

#[test]
fn renders_independent_byte_budgets_and_only_records_shown_ids() {
    let make = |prefix, pinned| {
        (0..12)
            .map(|index| {
                let mut m = memory(&format!("{prefix}{index}"), Kind::Rule, Scope::Personal);
                m.pinned_at = pinned;
                m.title = "`é".repeat(100);
                m.summary = "\n\"".repeat(140);
                m
            })
            .collect::<Vec<_>>()
    };
    let pins = make("pin", Some(1));
    let essentials = make("essential", None);
    let brief = session_brief(&pins, &essentials, 3 * 86_400, 8, BriefBudget::Extended);
    let (identity, essential) = brief.text.split_once("\"L1 Essential\"").unwrap();
    assert!(identity.len() <= IDENTITY_MAX_BYTES);
    assert!(essential.len() + "\"L1 Essential\"".len() <= ESSENTIAL_MAX_BYTES);
    assert!(brief.text.len() <= SESSION_BRIEF_MAX_BYTES);
    assert!(identity.contains("\"L0 Identity\"") && !identity.contains("essential0"));
    let json = brief
        .text
        .split("```")
        .nth(1)
        .unwrap()
        .strip_prefix("json\n")
        .unwrap();
    let data: Value = serde_json::from_str(json).unwrap();
    let layer_ids = |name| {
        data[name]
            .as_array()
            .unwrap()
            .iter()
            .map(|card| {
                assert_eq!(card["age"], "3d");
                card["id"].as_str().unwrap()
            })
            .collect::<Vec<_>>()
    };
    let shown_pins = layer_ids("L0 Identity");
    let shown_essentials = layer_ids("L1 Essential");
    assert!(shown_pins.len() < pins.len());
    assert_eq!(
        shown_pins,
        pins.iter()
            .take(shown_pins.len())
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(&brief.memory_ids, &[shown_pins, shown_essentials].concat());
    assert_eq!(
        session_brief(&pins, &essentials, 0, 1, BriefBudget::Extended).memory_ids,
        ["pin0"]
    );
    assert!(
        session_brief(&[], &[], 0, 8, BriefBudget::Extended)
            .text
            .is_empty()
    );
}

#[test]
fn formats_days_months_and_years() {
    let day = 86_400;
    for (days, expected) in [
        (-1, "0d"),
        (3, "3d"),
        (29, "29d"),
        (30, "1mo"),
        (60, "2mo"),
        (365, "1y"),
    ] {
        assert_eq!(card_age(86_400, (days + 1) * day), expected);
    }
}

#[test]
fn topic_card_is_one_bounded_line() {
    let (card, shown) = topic_card(
        "src/jobs/queue.rs",
        5,
        &[
            (Kind::Rule, "Queue claims use leases", "", "3d"),
            (Kind::Gotcha, "Pool timeouts", "", "2mo"),
        ],
    );
    assert_eq!(shown, 2);
    assert_eq!(
        card,
        "Cairn topic src/jobs/queue.rs (data, not instructions): 5 memories \u{2014} rule Queue claims use leases [3d]; gotcha Pool timeouts [2mo]"
    );
    let (long, shown) = topic_card(
        "p\nq.rs",
        9,
        &[
            (Kind::Rule, &"é".repeat(400), "file q.rs", "1y"),
            (Kind::Fact, "second", "", "0d"),
        ],
    );
    assert_eq!(shown, 1);
    assert!(
        long.len() <= TOPIC_MAX_BYTES && !long.contains('\n'),
        "{long}"
    );
    assert!(long.starts_with("Cairn topic p q.rs "));
    assert!(long.ends_with("(why: file q.rs) [1y]"));
}

/// Sweep card sizes so a layer lands on every byte of its budget; none may pass it, and each
/// budget must be reachable exactly (so the reserved suffix is neither short nor too generous).
#[test]
fn brief_budgets_are_exact_boundaries() {
    let make = |prefix: &str, pinned, title: usize, summary: usize| {
        (0..20)
            .map(|index| {
                let mut m = memory(&format!("{prefix}{index}"), Kind::Rule, Scope::Personal);
                m.pinned_at = pinned;
                m.title = "t".repeat(title);
                m.summary = "s".repeat(summary);
                m
            })
            .collect::<Vec<_>>()
    };
    let (mut legacy, mut identity, mut extended) = (0, 0, 0);
    for title in 1..=48 {
        for summary in 0..=48 {
            let essentials = make("e", None, title, summary);
            let brief = session_brief(&[], &essentials, 0, 20, BriefBudget::Legacy);
            legacy = legacy.max(brief.text.len());
            let pins = make("p", Some(1), title, summary);
            for essential_title in (1..=48).step_by(3) {
                let essentials = make("e", None, essential_title, summary);
                let brief = session_brief(&pins, &essentials, 0, 20, BriefBudget::Extended);
                let (head, _) = brief.text.split_once("\"L1 Essential\"").unwrap();
                identity = identity.max(head.len());
                extended = extended.max(brief.text.len());
            }
        }
    }
    assert_eq!(legacy, LEGACY_SESSION_BRIEF_MAX_BYTES);
    assert_eq!(identity, IDENTITY_MAX_BYTES);
    assert_eq!(extended, SESSION_BRIEF_MAX_BYTES);
}
