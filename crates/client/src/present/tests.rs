use super::*;

fn text_of(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

fn structured(result: &Value) -> &Value {
    &result["structuredContent"]
}

#[test]
fn save_receipt_is_three_fields_in_both_channels() {
    let result = present(
        "memory_save",
        false,
        json!({"id": "mem-1", "status": "active", "deduplicated": false, "title": "Hook auth"}),
    );
    let receipt = json!({"id": "mem-1", "status": "active", "deduplicated": false});
    assert_eq!(structured(&result), &receipt);
    assert_eq!(text_of(&result), compact(&receipt));
}

#[test]
fn search_defaults_to_cards_and_expanded_drops_internals() {
    let hit = json!({
        "id": "mem-1",
        "owner": "person-1",
        "kind": "gotcha",
        "scope": "project",
        "project_id": "proj-1",
        "title": "Hook auth",
        "summary": "401 names the config file.",
        "score": 0.5,
        "status": "active",
        "reason": "Agents must see the file path.",
        "fields": {"symptom": "401", "fix": "name the file"},
        "evidence": ["crates/client/src/lib.rs"],
        "evidence_count": 1,
        "entities": [],
        "links": [],
        "context": {},
        "created_at": 1,
        "updated_at": 2
    });
    let card = present("memory_search", false, json!([hit.clone()]));
    assert_eq!(
        structured(&card)["results"][0],
        json!({
            "id": "mem-1",
            "kind": "gotcha",
            "scope": "project",
            "title": "Hook auth",
            "summary": "401 names the config file.",
            "score": 0.5
        })
    );
    assert_eq!(text_of(&card), compact(structured(&card)));
    assert!(structured(&card).is_object());

    let empty = present("memory_smart_search", false, json!([]));
    assert_eq!(structured(&empty), &json!({"results": []}));

    let pack = present(
        "memory_recall",
        false,
        json!({"query": "auth", "results": [hit.clone()], "more": 2}),
    );
    assert_eq!(structured(&pack)["more"], 2);
    assert_eq!(structured(&pack)["results"][0]["kind"], "gotcha");
    assert_eq!(text_of(&pack), compact(structured(&pack)));

    let expanded = present("memory_context", true, json!([hit]));
    let row = &structured(&expanded)["results"][0];
    assert_eq!(row["reason"], "Agents must see the file path.");
    assert_eq!(row["fields"]["fix"], "name the file");
    assert_eq!(row["evidence"], json!(["crates/client/src/lib.rs"]));
    assert_eq!(row["project_id"], "proj-1");
    for key in [
        "owner",
        "evidence_count",
        "created_at",
        "entities",
        "links",
        "context",
    ] {
        assert!(row.get(key).is_none(), "{key}");
    }
    assert_eq!(text_of(&expanded), compact(structured(&expanded)));
}

#[test]
fn default_cards_clip_utf8_and_get_keeps_the_text() {
    let snow = "雪".repeat(81);
    assert_eq!(snow.len(), 243);
    let mut split = "a".repeat(116);
    split.push('雪');
    split.push('b');
    assert_eq!(split.len(), 120);
    let title = format!("{split}b");
    assert_eq!(title.len(), 121);
    let summary = snow;
    let hit = json!({
        "id": "mem-1",
        "kind": "note",
        "scope": "project",
        "title": title,
        "summary": summary,
        "score": 0
    });
    let card = present("memory_search", false, json!([hit.clone()]));
    let row = &structured(&card)["results"][0];
    let card_title = row["title"].as_str().unwrap();
    let card_summary = row["summary"].as_str().unwrap();
    assert!(card_title.ends_with('…'));
    assert!(card_summary.ends_with('…'));
    assert!(card_title.len() <= 120);
    assert!(card_summary.len() <= 240);
    assert!(card_title.is_char_boundary(card_title.len()));
    assert!(!card_title.contains('\u{FFFD}'));
    assert_eq!(&card_title[..116], &title[..116]);
    assert_eq!(text_of(&card), compact(structured(&card)));

    let expanded = present("memory_recall", true, json!([hit.clone()]));
    assert_eq!(structured(&expanded)["results"][0]["title"], title);
    assert_eq!(structured(&expanded)["results"][0]["summary"], summary);
    assert_eq!(structured(&expanded)["results"][0]["score"], 0);

    let mut record = hit;
    record["fields"] = json!({"source": "docs"});
    record["evidence"] = json!([]);
    let loaded = present("memory_get", false, record.clone());
    assert_eq!(structured(&loaded)["title"], title);
    assert_eq!(structured(&loaded)["summary"], summary);
    assert_eq!(structured(&loaded)["evidence"], json!([]));
    assert_eq!(text_of(&loaded), compact(&record));
}

#[test]
fn expanded_omits_empty_lists_and_objects_but_keeps_empty_results() {
    let hit = json!({
        "id": "mem-1",
        "kind": "rule",
        "scope": "project",
        "title": "Short",
        "summary": "Kept.",
        "score": 1,
        "fields": {},
        "entities": [],
        "links": [],
        "evidence": ["crates/client/src/present.rs"]
    });
    let expanded = present(
        "memory_context",
        true,
        json!({"query": "short", "results": [hit]}),
    );
    let row = &structured(&expanded)["results"][0];
    for key in ["fields", "entities", "links"] {
        assert!(row.get(key).is_none(), "{key}");
    }
    assert_eq!(row["evidence"], json!(["crates/client/src/present.rs"]));
    assert_eq!(text_of(&expanded), compact(structured(&expanded)));
    let none = present(
        "memory_context",
        false,
        json!({"query": "x", "results": []}),
    );
    assert_eq!(structured(&none)["results"], json!([]));

    let why = present(
        "memory_why_changed",
        false,
        json!({"query": "short", "results": [{"id": "mem-1", "links": [], "context": {"branch": "main"}}]}),
    );
    assert!(structured(&why)["results"][0].get("links").is_none());
    assert_eq!(structured(&why)["results"][0]["context"]["branch"], "main");
}

#[test]
fn lists_wrap_empty_arrays_and_get_stays_complete() {
    let listed = present("memory_list", false, json!([]));
    assert_eq!(structured(&listed), &json!({"memories": []}));
    let record = json!({
        "id": "mem-1",
        "kind": "fact",
        "fields": {"source": "docs"},
        "evidence": [],
        "links": [],
        "reason": ""
    });
    let loaded = present("memory_get", false, record.clone());
    assert_eq!(structured(&loaded), &record);
    assert_eq!(text_of(&loaded), compact(&record));
    assert!(!text_of(&loaded).contains('\n'));
}

#[test]
fn ordinary_json_is_compact_and_drops_only_empty_optionals() {
    let result = present(
        "memory_diagnose",
        false,
        json!({"temporary_observations": 0, "note": "", "ready": false, "extra": null}),
    );
    assert_eq!(
        structured(&result),
        &json!({"temporary_observations": 0, "ready": false})
    );
    assert_eq!(text_of(&result), compact(structured(&result)));
}

#[test]
fn graph_uses_toon_text_and_json_objects() {
    let graph = json!({
        "nodes": [{
            "memory_id": "mem-1",
            "title": "say \"hi\",\n\tok café 雪",
            "scope": "project",
            "project_id": null,
            "kind": "fact"
        }, {
            "memory_id": "mem-2",
            "title": "null",
            "scope": "personal",
            "project_id": null,
            "kind": "-note"
        }],
        "edges": [{
            "source_id": "mem-1",
            "target_id": "mem-2",
            "relation": "fixes \"a\"\n雪"
        }],
        "truncated": false,
        "node_cap": 120
    });
    let presented = present("memory_graph", false, graph);
    assert_eq!(
        text_of(&presented),
        "edges[1\t]{relation\tsource_id\ttarget_id}:\n  \
         \"fixes \\\"a\\\"\\n雪\"\tmem-1\tmem-2\n\
         node_cap: 120\n\
         nodes[2\t]{kind\tmemory_id\tproject_id\tscope\ttitle}:\n  \
         fact\tmem-1\tnull\tproject\t\"say \\\"hi\\\",\\n\\tok café 雪\"\n  \
         \"-note\"\tmem-2\tnull\tpersonal\t\"null\"\n\
         truncated: false"
    );
    assert!(!text_of(&presented).ends_with('\n'));
    assert!(
        structured(&presented)["nodes"][0]
            .get("project_id")
            .is_none()
    );
    assert_eq!(structured(&presented)["truncated"], false);
    assert_eq!(
        structured(&presented)["edges"],
        json!([{"relation": "fixes \"a\"\n雪", "source_id": "mem-1", "target_id": "mem-2"}])
    );
    assert_eq!(structured(&presented)["node_cap"], 120);
}

#[test]
fn toon_quotes_delimiter_controls_and_keeps_unicode_plain() {
    let value = json!({
        "nodes": [{
            "memory_id": "m",
            "title": "a\tb",
            "scope": "05",
            "project_id": "true",
            "kind": "café 雪"
        }],
        "edges": [],
        "truncated": false,
        "node_cap": 0
    });
    let text = toon_document(&value).unwrap();
    assert_eq!(
        text,
        "edges: []\n\
         node_cap: 0\n\
         nodes[1\t]{kind\tmemory_id\tproject_id\tscope\ttitle}:\n  \
         café 雪\tm\t\"true\"\t\"05\"\t\"a\\tb\"\n\
         truncated: false"
    );
}
