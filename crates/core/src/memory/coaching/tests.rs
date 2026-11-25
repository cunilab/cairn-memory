use super::*;
use serde_json::json;

const TITLE: &str = "Webhook handlers suppress repeat deliveries";
const SUMMARY: &str = "An event ID table prevents double billing.";
const REASON: &str = "Providers may retry after timing out.";
const REASON_HINT: &str =
    "Reason should explain why this is true or was chosen in at least 20 characters.";
const TITLE_HINT: &str = "Use at least three specific words in the title to name the lesson.";
const SUMMARY_HINT: &str = "Write a summary of at least five words that adds to the title.";

fn fields(kind: Kind, value: serde_json::Value) -> Fields {
    Fields::parse(kind, Some(value)).unwrap()
}

#[test]
fn title_summary_and_reason_coaching() {
    let fact = fields(Kind::Fact, json!({}));
    let cases = [
        (TITLE, SUMMARY, REASON, vec![]),
        ("Queue retries", SUMMARY, REASON, vec![TITLE_HINT]),
        ("BUG, Fix TODO.", SUMMARY, REASON, vec![TITLE_HINT]),
        (
            TITLE,
            "Billing never repeats twice",
            REASON,
            vec![SUMMARY_HINT],
        ),
        (
            TITLE,
            "WEBHOOK handlers suppress repeat deliveries!",
            REASON,
            vec![SUMMARY_HINT],
        ),
        (
            "Why webhook handlers suppress repeat deliveries here",
            TITLE,
            REASON,
            vec![SUMMARY_HINT],
        ),
        (TITLE, SUMMARY, "A short reason", vec![REASON_HINT]),
        (TITLE, SUMMARY, "   ", vec![REASON_HINT]),
        (
            TITLE,
            SUMMARY,
            "Webhook HANDLERS suppress repeat deliveries!",
            vec![REASON_HINT],
        ),
        (
            TITLE,
            SUMMARY,
            "an event id table prevents double billing",
            vec![REASON_HINT],
        ),
        (TITLE, SUMMARY, "12345678901234567890", vec![]),
    ];
    for (title, summary, reason, expected) in cases {
        assert_eq!(
            save_hints(title, summary, reason, &fact, None),
            expected,
            "{title} / {summary} / {reason}"
        );
    }
    let note = fields(Kind::Note, json!({}));
    assert!(save_hints(TITLE, SUMMARY, "", &note, None).is_empty());
    assert_eq!(
        save_hints(TITLE, SUMMARY, "short", &note, None),
        [REASON_HINT]
    );
}

#[test]
fn kind_fields_coaching() {
    let gotcha_hint = "Fill in the gotcha's symptom, cause and fix.";
    let decision_hint = "Add decision context or list the alternatives you considered.";
    let steps_hint = "List the steps of the procedure in order.";
    let cases = [
        (
            Kind::Gotcha,
            json!({"cause":"Retries", "fix":"Dedupe IDs"}),
            Some(gotcha_hint),
        ),
        (
            Kind::Gotcha,
            json!({"symptom":"Two bills", "fix":"Dedupe IDs"}),
            Some(gotcha_hint),
        ),
        (
            Kind::Gotcha,
            json!({"symptom":"Two bills", "cause":"Retries"}),
            Some(gotcha_hint),
        ),
        (
            Kind::Gotcha,
            json!({"symptom":"Two bills", "cause":"Retries", "fix":"Dedupe IDs"}),
            None,
        ),
        (
            Kind::Decision,
            json!({"context":" ", "alternatives":[" "]}),
            Some(decision_hint),
        ),
        (Kind::Decision, json!({"context":"Clients retry"}), None),
        (
            Kind::Decision,
            json!({"alternatives":["Make delivery synchronous"]}),
            None,
        ),
        (Kind::Procedure, json!({"steps":[" "]}), Some(steps_hint)),
        (Kind::Procedure, json!({"steps":["Open account"]}), None),
        (Kind::Rule, json!({}), None),
        (Kind::Preference, json!({}), None),
    ];
    for (kind, value, expected) in cases {
        assert_eq!(
            save_hints(TITLE, SUMMARY, REASON, &fields(kind, value), None),
            expected.into_iter().collect::<Vec<_>>(),
            "{kind:?}"
        );
    }
}

#[test]
fn duplicate_hint_is_bounded_and_redacts_before_truncating() {
    let id = "mem_abcdefghijklmnopqrstuvwxyz";
    let gotcha = fields(Kind::Gotcha, json!({}));
    let title = format!("password=fake-secret {}", "ä".repeat(65));
    let hints = save_hints("fix", "fix", "fix", &gotcha, Some((id, &title)));
    assert_eq!(hints.len(), 4);
    assert!(hints.iter().all(|hint| hint.len() <= MAX_HINT_BYTES));
    assert!(hints[0].starts_with(&format!("Looks like {id} 'password=[REDACTED]")));
    assert!(hints[0].ends_with(&format!("Use supersedes={id}.")));
    assert!(!hints[0].contains("fake-secret"));
    assert_eq!(&hints[1..], [REASON_HINT, TITLE_HINT, SUMMARY_HINT]);
}

#[test]
fn system_handoffs_skip_coaching_entirely() {
    let fields = fields(Kind::Handoff, json!({}));
    assert!(save_hints("fix", "fix", "", &fields, Some(("mem_fake", "fix"))).is_empty());
}
