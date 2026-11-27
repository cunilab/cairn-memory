use super::*;
use crate::memory::Scope;
use crate::testing::memory;
use serde_json::Value;

fn decode_brief(text: &str) -> Value {
    let json = text
        .split("```")
        .nth(1)
        .unwrap()
        .strip_prefix("json\n")
        .unwrap();
    serde_json::from_str(json).unwrap()
}

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
    let brief = session_brief(&pins, &essentials, 3 * 86_400, 20, BriefBudget::Extended);
    let (identity, essential) = brief.text.split_once("\"L1 Essential\"").unwrap();
    assert!(identity.len() <= IDENTITY_MAX_BYTES, "{}", identity.len());
    assert!(
        essential.len() + "\"L1 Essential\"".len() <= ESSENTIAL_MAX_BYTES,
        "{}",
        essential.len()
    );
    assert!(
        brief.text.len() <= SESSION_BRIEF_MAX_BYTES,
        "{}",
        brief.text.len()
    );
    assert!(identity.contains("\"L0 Identity\"") && !identity.contains("essential0"));
    let data = decode_brief(&brief.text);
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
    assert_eq!(brief.pins_not_shown, pins.len() - shown_pins.len());
    assert_eq!(
        data["L0_not_shown"],
        format!("{} more pinned not shown", brief.pins_not_shown)
    );
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
    assert!(
        session_brief(&pins, &essentials, 0, 0, BriefBudget::Extended)
            .text
            .is_empty()
    );
    // Unused L0 room in Extended does not allow L1 to grow past ESSENTIAL_MAX_BYTES.
    let no_pins = session_brief(&[], &essentials, 3 * 86_400, 20, BriefBudget::Extended);
    let (_, no_pins_l1) = no_pins.text.split_once("\"L1 Essential\"").unwrap();
    assert!(
        no_pins_l1.len() + "\"L1 Essential\"".len() <= ESSENTIAL_MAX_BYTES,
        "{}",
        no_pins_l1.len()
    );
    assert_eq!(no_pins.pins_not_shown, 0);
}

#[test]
fn l0_splits_personal_and_project_equally_with_spillover() {
    let make = |prefix, scope, count| {
        (0..count)
            .map(|index| {
                let mut m = memory(&format!("{prefix}{index}"), Kind::Rule, scope);
                m.pinned_at = Some(index as i64 + 1);
                m.title = format!("{prefix} rule {index}");
                m.summary = "Summary text long enough to make each card substantial.".repeat(3);
                m
            })
            .collect::<Vec<_>>()
    };

    // Older personal pins cannot crowd out project pins.
    let mut mixed = make("per", Scope::Personal, 3);
    mixed.extend(make("prj", Scope::Project, 4));
    let brief = session_brief(&mixed, &[], 0, usize::MAX, BriefBudget::Extended);
    assert!(brief.text.len() <= SESSION_BRIEF_MAX_BYTES);
    let data = decode_brief(&brief.text);
    let l0: Vec<&str> = data["L0 Identity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(l0.iter().any(|id| id.starts_with("per")), "{l0:?}");
    assert!(l0.iter().any(|id| id.starts_with("prj")), "{l0:?}");
    assert_eq!(brief.pins_not_shown, mixed.len() - l0.len());
    if brief.pins_not_shown > 0 {
        assert_eq!(
            data["L0_not_shown"],
            format!("{} more pinned not shown", brief.pins_not_shown)
        );
    }

    // When only one scope has pins, unused half spills over so all compact pins fit.
    let personal_only = make("onlyp", Scope::Personal, 3);
    let fit = l0_pin_fit(&personal_only, 0, BriefBudget::Extended);
    assert_eq!(fit.personal_shown, 3);
    assert_eq!(fit.personal_total, 3);
    assert!(fit.omitted_ids.is_empty());
    let single_scope = session_brief(&personal_only, &[], 0, usize::MAX, BriefBudget::Extended);
    let single_data = decode_brief(&single_scope.text);
    assert!(single_data.get("L0_not_shown").is_none());

    // Legacy 600B still guarantees room for both scopes when both exist.
    let legacy = session_brief(&mixed, &[], 0, usize::MAX, BriefBudget::Legacy);
    assert!(legacy.text.len() <= LEGACY_SESSION_BRIEF_MAX_BYTES);
    let legacy_data = decode_brief(&legacy.text);
    let legacy_l0: Vec<&str> = legacy_data["L0 Identity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(
        legacy_l0.iter().any(|id| id.starts_with("per")),
        "{legacy_l0:?}"
    );
    assert!(
        legacy_l0.iter().any(|id| id.starts_with("prj")),
        "{legacy_l0:?}"
    );
    assert_eq!(legacy.pins_not_shown, mixed.len() - legacy_l0.len());
}

#[test]
fn l1_labels_personal_rule_distinct_from_project_rule() {
    let mut personal_rule = memory("prule", Kind::Rule, Scope::Personal);
    personal_rule.title = "Priority rule".into();
    let mut project_rule = memory("jrule", Kind::Rule, Scope::Project);
    project_rule.title = "Project rule".into();
    let brief = session_brief(
        &[],
        &[personal_rule, project_rule],
        0,
        usize::MAX,
        BriefBudget::Extended,
    );
    let data = decode_brief(&brief.text);
    let l1 = data["L1 Essential"].as_array().unwrap();
    assert_eq!(l1[0]["why"], "personal rule");
    assert_eq!(l1[1]["why"], "project rule");
}

#[test]
fn pin_limits_match_quotas() {
    assert_eq!(pin_limit(Scope::Personal), Some(3));
    assert_eq!(pin_limit(Scope::Project), Some(4));
    assert_eq!(pin_limit(Scope::Global), None);
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
