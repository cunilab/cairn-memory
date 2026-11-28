use super::*;
use serde_json::json;

fn check(value: serde_json::Value) -> Result<(), Error> {
    let input: SaveInput = serde_json::from_value(value).expect("shape");
    let fields = Fields::parse(input.kind, input.fields.clone()).expect("fields");
    validate_quality(&input, &fields)
}

fn base(kind: &str) -> serde_json::Value {
    json!({"kind": kind, "title": "Webhook handlers dedupe", "summary": "Handlers keep an event id table so replays do nothing.", "reason": "Providers retry deliveries."})
}

fn with(kind: &str, key: &str, value: serde_json::Value) -> serde_json::Value {
    let mut v = base(kind);
    v[key] = value;
    v
}

#[test]
fn save_length_errors_give_the_actual_size_and_cut() {
    let cases = [
        (
            with("rule", "title", json!("é".repeat(201))),
            "title is 201 characters, limit 200: cut 1",
        ),
        (
            with("rule", "summary", json!("x".repeat(1001))),
            "summary is 1001 characters, limit 1000: cut 1, or move extra context into reason or detail",
        ),
        (
            with("rule", "reason", json!("x".repeat(1002))),
            "reason is 1002 characters, limit 1000: cut 2, or move background into detail",
        ),
        (
            with("rule", "detail", json!("x".repeat(8003))),
            "detail is 8003 characters, limit 8000: cut 3",
        ),
        (
            with("rule", "fields", json!({"exceptions": "x".repeat(1004)})),
            "fields.exceptions is 1004 characters, limit 1000: cut 4",
        ),
        (
            with(
                "procedure",
                "fields",
                json!({"steps": ["one", "x".repeat(305)]}),
            ),
            "fields.steps[1] is 305 characters, limit 300: cut 5",
        ),
        (with("rule", "title", json!(" ")), "title must not be empty"),
        (
            with("rule", "summary", json!("")),
            "summary must not be empty",
        ),
        (
            with("rule", "reason", json!("")),
            "reason must not be empty for kind rule",
        ),
    ];
    for (value, message) in cases {
        let input: SaveInput = serde_json::from_value(value).expect("shape");
        let fields = Fields::parse(input.kind, input.fields.clone()).expect("fields");
        assert_eq!(
            validate_save(&input, &fields, false),
            Err(Error::Validation(message.to_owned()))
        );
    }
}

#[test]
fn sizes_over_the_suggestion_save_as_sent_with_a_hint() {
    let value = json!({"kind": "rule", "title": "t".repeat(200), "summary": "s".repeat(1000), "reason": "Providers retry deliveries."});
    let input: SaveInput = serde_json::from_value(value).expect("shape");
    let fields = Fields::parse(input.kind, input.fields.clone()).expect("fields");
    assert_eq!(validate_save(&input, &fields, false), Ok(()));
    assert_eq!(validate_quality(&input, &fields), Ok(()));
    assert_eq!(
        length_hints("t".repeat(81).as_str(), "s".repeat(264).as_str()),
        [
            "title is 81 characters; 80 suggested",
            "summary is 264 characters; 240 suggested"
        ]
    );
    assert!(length_hints(&"t".repeat(80), &"é".repeat(240)).is_empty());
}

#[test]
fn cards_cut_to_the_suggested_size_on_a_character_boundary() {
    assert_eq!(card_text("short", 80), "short");
    assert_eq!(card_text(&"é".repeat(80), 80), "é".repeat(80));
    let long = "é".repeat(81);
    let cut = card_text(&long, 80);
    assert_eq!(cut.chars().count(), 80);
    assert_eq!(cut, format!("{}\u{2026}", "é".repeat(79)));
}

#[test]
fn quality_messages() {
    let mut gotcha = base("gotcha");
    gotcha["fields"] = json!({"fix": "dedupe by event id"});
    let mut procedure = base("procedure");
    procedure["fields"] = json!({"steps": ["a", "b"]});
    for ok in [
        base("decision"),
        gotcha.clone(),
        procedure.clone(),
        with("note", "reason", json!("")),
    ] {
        assert_eq!(check(ok.clone()), Ok(()), "{ok}");
    }
    let cases = [
        (
            with("rule", "title", json!("short")),
            "title: use 8-200 characters (about 80) that name the knowledge",
        ),
        (
            with("rule", "title", json!("x".repeat(201))),
            "title: use 8-200 characters (about 80) that name the knowledge",
        ),
        (
            with("rule", "summary", json!("short summary")),
            "summary: use 20-1000 characters (about 240) saying what is true",
        ),
        (
            with("rule", "summary", json!("x".repeat(1001))),
            "summary: use 20-1000 characters (about 240) saying what is true",
        ),
        (
            {
                let mut v = base("rule");
                v["summary"] = json!("WEBHOOK handlers dedupe!!  ");
                v["title"] = json!("Webhook handlers dedupe");
                v["summary"] = json!("webhook handlers dedupe.");
                v
            },
            "summary: repeats the title; say what is true in 1-3 sentences",
        ),
        (
            with("rule", "reason", json!("because")),
            "reason: say why this is true or was chosen (10+ characters)",
        ),
        (base("gotcha"), "fields.fix: say how the gotcha was fixed"),
        (
            with("procedure", "fields", json!({"steps": ["only one"]})),
            "fields.steps: list at least 2 ordered steps",
        ),
    ];
    for (input, message) in cases {
        assert_eq!(
            check(input.clone()),
            Err(Error::Quality(message.to_owned())),
            "{input}"
        );
    }
    // the first failing rule wins
    let mut both = with("rule", "title", json!("short"));
    both["summary"] = json!("short");
    assert!(matches!(check(both), Err(Error::Quality(m)) if m.starts_with("title:")));
}
