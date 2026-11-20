//! The agent tool surface. Each tool is one REST call; the `cairn` CLI and the MCP server
//! share this table, so a command and its tool cannot drift apart.
//!
//! Web-only actions (account and device management) have no tool: a device token is refused
//! there, so offering them would only invite retries.

use crate::{Client, Error};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Post,
}

pub struct Tool {
    pub name: &'static str,
    description: &'static str,
    method: Method,
    /// `{name}` segments are filled from the argument of that name and not sent in the body.
    path: &'static str,
    schema: Value,
}

impl Tool {
    pub fn definition(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.schema
        })
    }
}

pub fn find(name: &str) -> Option<&'static Tool> {
    tools().iter().find(|tool| tool.name == name)
}

pub fn tools() -> &'static [Tool] {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(build_tools)
}

/// Runs one tool against the server and returns the server's JSON result.
pub fn call(client: &Client, tool: &Tool, args: &Map<String, Value>) -> Result<Value, Error> {
    let required = tool.schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for key in required {
        let present = match args.get(key) {
            None | Some(Value::Null) => false,
            Some(Value::String(text)) => !text.trim().is_empty(),
            Some(_) => true,
        };
        if !present {
            return Err(Error::Input(format!("{key} is required")));
        }
    }
    let mut body = args.clone();
    if matches!(
        tool.name,
        "memory_recall"
            | "memory_context"
            | "memory_reflect"
            | "memory_smart_search"
            | "memory_search"
            | "memory_why_changed"
            | "memory_resume"
            | "memory_unresolved"
            | "memory_session_start"
    ) && let Ok(cwd) = std::env::current_dir()
        && let Some(top) = crate::project::toplevel(&cwd)
    {
        let repository = crate::project::repository_name(&top);
        let context = body.entry("context").or_insert_with(|| json!({}));
        if let Some(context) = context.as_object_mut() {
            let same_repo = context
                .get("repository")
                .and_then(Value::as_str)
                .is_none_or(|r| r == repository);
            if same_repo {
                context
                    .entry("repository")
                    .or_insert_with(|| json!(repository));
                if let Some(id) = crate::project::project_id(&cwd) {
                    context.entry("project_id").or_insert_with(|| json!(id));
                }
            }
        }
    }
    let mut attachment_warning = None;
    if tool.name == "memory_observe"
        && body
            .get("context")
            .and_then(|c| c.get("project_id"))
            .and_then(Value::as_str)
            .is_none()
        && let Ok(cwd) = std::env::current_dir()
    {
        match crate::project::read_project_id(&cwd) {
            Ok(Some(id)) => {
                let context = body.entry("context").or_insert_with(|| json!({}));
                if let Some(context) = context.as_object_mut() {
                    context.entry("project_id").or_insert_with(|| json!(id));
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("cairn: warning: {error}");
                attachment_warning = Some(error.to_string());
            }
        }
    }
    // The server defaults a save to project scope when the context names a project.
    if matches!(tool.name, "memory_save" | "memory_lesson_save")
        && body.get("scope").and_then(Value::as_str) != Some("global")
        && let Ok(cwd) = std::env::current_dir()
        && let Some(id) = crate::project::project_id(&cwd)
    {
        let context = body.entry("context").or_insert_with(|| json!({}));
        if let Some(context) = context.as_object_mut() {
            context.entry("project_id").or_insert_with(|| json!(id));
        }
    }
    let path = fill_path(tool.path, &mut body)?;
    let graph_query = (tool.name == "memory_graph").then(|| query_string(&body));
    if tool.name == "memory_lesson_save" {
        body.entry("kind").or_insert_with(|| json!("gotcha"));
    }
    let route = match graph_query.as_deref() {
        Some(query) if !query.is_empty() => format!("/cairn-memory{path}?{query}"),
        _ => format!("/cairn-memory{path}"),
    };
    let mut result = match tool.method {
        Method::Get => client.get(&route),
        Method::Post => client.post(&route, &Value::Object(body)),
    }?;
    if let Some(warning) = attachment_warning {
        result["project_attachment"] = json!({"status": "personal", "warning": warning});
    }
    if tool.name == "memory_reflect" {
        return Ok(json!({
            "query": result["query"],
            "results": result["results"],
            "more": result["more"],
            "mode": "retrieval-only",
            "note": "No model-generated reflection is performed."
        }));
    }
    Ok(result)
}

pub fn fill_path(template: &str, body: &mut Map<String, Value>) -> Result<String, Error> {
    let mut path = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = rest[start..]
            .find('}')
            .map_or(rest.len(), |offset| start + offset);
        let key = &rest[start + 1..end];
        let value = body
            .remove(key)
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| Error::Input(format!("{key} is required")))?;
        // `.` and `..` survive percent-encoding and would climb out of the route.
        if matches!(value.as_str(), "." | "..") {
            return Err(Error::Input(format!("{key} must not be \".\" or \"..\"")));
        }
        path.push_str(&rest[..start]);
        path.extend(value.bytes().map(encode_byte));
        rest = rest.get(end + 1..).unwrap_or("");
    }
    path.push_str(rest);
    Ok(path)
}

fn query_string(body: &Map<String, Value>) -> String {
    let mut pairs = Vec::new();
    for (key, value) in body {
        let Some(text) = query_value(value) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        pairs.push(format!(
            "{}={}",
            encode_component(key),
            encode_component(&text)
        ));
    }
    pairs.join("&")
}

fn query_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        encoded.push_str(&encode_byte(byte));
    }
    encoded
}

fn encode_byte(byte: u8) -> String {
    match byte {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
            char::from(byte).to_string()
        }
        _ => format!("%{byte:02X}"),
    }
}

fn build_tools() -> Vec<Tool> {
    // The `SaveInput` shape: the server derives everything else and rejects unknown keys.
    let save_schema = |kind_required: bool| {
        let properties = [
            (
                "kind",
                json!({"type":"string", "enum":["decision","rule","gotcha","procedure","fact","preference","note"]}),
            ),
            ("title", string_schema()),
            ("summary", string_schema()),
            ("reason", string_schema()),
            (
                "fields",
                json!({"type":"object", "description": FIELDS_HELP}),
            ),
            ("evidence", array_schema()),
            ("supersedes", string_schema()),
            ("contradicts", string_schema()),
            (
                "scope",
                json!({"type":"string", "enum":["project", "personal", "global"], "description": "Optional. Default: project in an initialized repository, else personal. A preference is always personal."}),
            ),
            ("context", json!({"type":"object"})),
        ];
        let required: &[&str] = if kind_required {
            &["kind", "title", "summary", "reason"]
        } else {
            &["title", "summary", "reason"]
        };
        object_schema(&properties, required)
    };
    let project_schema = || object_schema(&[("projectId", string_schema())], &["projectId"]);
    use Method::{Get, Post};
    let spec = |name, description, method, path, schema| Tool {
        name,
        description,
        method,
        path,
        schema,
    };
    vec![
        spec(
            "memory_save",
            "Cairn. Save verified reusable knowledge at decisions, corrections and task completion. Required: kind/title/summary (<=240 chars)/reason. Scope defaults to project in an initialized repository, else personal; global is visible to every server user. Fill the kind's fields and evidence when known; a correction is a save with supersedes. Avoid duplicates and secrets. The MCP result is id, status and deduplicated.",
            Post,
            "/remember",
            save_schema(true),
        ),
        spec(
            "memory_recall",
            "Cairn. Recall a bounded context pack. Results default to short cards; set expanded true for recall fields without lifecycle, internal ids, or duplicated content.",
            Post,
            "/recall",
            card_query_schema(),
        ),
        spec(
            "memory_consolidate",
            "Consolidate a captured session into an evidence-backed memory.",
            Post,
            "/session/end",
            session_schema(),
        ),
        spec(
            "memory_smart_search",
            "Cairn. Search durable memories. Returns short cards unless expanded is true.",
            Post,
            "/smart-search",
            card_query_schema(),
        ),
        spec(
            "memory_why_changed",
            "Recover decisions, reasons, evidence and links, including what replaced or contradicts what.",
            Post,
            "/why-changed",
            query_schema(),
        ),
        spec(
            "memory_diagnose",
            "Report memory counts by kind, status and scope, and the job queue.",
            Get,
            "/diagnose",
            object_schema(&[], &[]),
        ),
        spec(
            "memory_lesson_save",
            "Cairn. Save a verified lesson after a correction or resolved failure. Required: title/summary/reason. Kind defaults to gotcha: fill fields symptom/cause/fix and evidence when known; never invent causal fields. The MCP result is id, status and deduplicated.",
            Post,
            "/remember",
            save_schema(false),
        ),
        spec(
            "memory_reflect",
            "Cairn. Return retrieval candidates for reflection without invoking a model. Results default to short cards; set expanded true for recall fields without lifecycle, internal ids, or duplicated content.",
            Post,
            "/context",
            card_query_schema(),
        ),
        spec(
            "memory_session_start",
            "Start or resume a capture session.",
            Post,
            "/session/start",
            session_schema(),
        ),
        spec(
            "memory_observe",
            "Capture one bounded, replay-safe observation.",
            Post,
            "/observe",
            object_schema(
                &[
                    ("eventId", string_schema()),
                    ("sessionId", string_schema()),
                    ("content", string_schema()),
                    ("role", string_schema()),
                    ("source", string_schema()),
                ],
                &["eventId", "sessionId", "content", "role", "source"],
            ),
        ),
        spec(
            "memory_session_end",
            "Finalize a capture session; its notes and handoff are derived in the background.",
            Post,
            "/session/end",
            session_schema(),
        ),
        spec(
            "memory_search",
            "Cairn. Search durable memories. Returns short cards unless expanded is true.",
            Post,
            "/search",
            card_query_schema(),
        ),
        spec(
            "memory_context",
            "Cairn. Assemble a bounded context pack. Results default to short cards; set expanded true for recall fields without lifecycle, internal ids, or duplicated content.",
            Post,
            "/context",
            card_query_schema(),
        ),
        spec(
            "memory_resume",
            "Recover recent handoffs for the current work context.",
            Post,
            "/resume",
            continuity_schema(),
        ),
        spec(
            "memory_unresolved",
            "Recover recent handoffs with next steps or open questions.",
            Post,
            "/unresolved",
            continuity_schema(),
        ),
        spec(
            "memory_forget",
            "Forget a memory you own: its text is erased and future recall is suppressed.",
            Post,
            "/forget",
            object_schema(
                &[
                    ("id", string_schema()),
                    ("eventId", string_schema()),
                    ("memoryId", string_schema()),
                ],
                &[],
            ),
        ),
        spec(
            "memory_export",
            "Export the memories and links the account can see.",
            Post,
            "/export",
            object_schema(&[], &[]),
        ),
        spec(
            "memory_project_list",
            "List the account's projects and its role in each.",
            Get,
            "/projects",
            object_schema(&[], &[]),
        ),
        spec(
            "memory_project_register",
            "Initialize a shared project for a repository.",
            Post,
            "/projects/init",
            object_schema(
                &[("repository", string_schema()), ("name", string_schema())],
                &["repository", "name"],
            ),
        ),
        spec(
            "memory_list",
            "List readable project, personal and global memories.",
            Get,
            "/memories",
            object_schema(&[], &[]),
        ),
        spec(
            "memory_get",
            "Cairn. Read every field of a visible memory, including fields, evidence, entities and links.",
            Get,
            "/memories/{memoryId}",
            object_schema(&[("memoryId", string_schema())], &["memoryId"]),
        ),
        spec(
            "memory_contradict",
            "Declare that one visible memory contradicts another. Both stay visible and the project shows the conflict.",
            Post,
            "/contradict",
            object_schema(
                &[
                    ("contradictingId", string_schema()),
                    ("contradictedId", string_schema()),
                ],
                &["contradictingId", "contradictedId"],
            ),
        ),
        spec(
            "memory_supersede",
            "Mark one memory as superseding another. The older memory leaves normal recall and stays in history. You must own the older one, or both must be in your project.",
            Post,
            "/supersede",
            object_schema(
                &[
                    ("successorId", string_schema()),
                    ("targetId", string_schema()),
                ],
                &["successorId", "targetId"],
            ),
        ),
        spec(
            "memory_link",
            "Link two visible memories. Relations: relates, causes, fixes, depends_on. You must own the source memory.",
            Post,
            "/links",
            object_schema(
                &[
                    ("sourceId", string_schema()),
                    (
                        "relation",
                        json!({"type":"string","enum":["relates","causes","fixes","depends_on"]}),
                    ),
                    ("targetId", string_schema()),
                ],
                &["sourceId", "relation", "targetId"],
            ),
        ),
        spec(
            "memory_unlink",
            "Remove one typed link. Same rule as memory_link.",
            Post,
            "/links/remove",
            object_schema(
                &[
                    ("sourceId", string_schema()),
                    (
                        "relation",
                        json!({"type":"string","enum":["relates","causes","fixes","depends_on"]}),
                    ),
                    ("targetId", string_schema()),
                ],
                &["sourceId", "relation", "targetId"],
            ),
        ),
        spec(
            "memory_graph",
            "Read the memory link graph the caller can see. Optional scope, projectId, relation (relates, causes, fixes, depends_on, contradicts, supersedes) and cap from 120 through 500 (default 120).",
            Get,
            "/graph",
            object_schema(
                &[
                    (
                        "scope",
                        json!({"type":"string","enum":["project","personal","global"]}),
                    ),
                    ("projectId", string_schema()),
                    (
                        "relation",
                        json!({"type":"string","enum":["relates","causes","fixes","depends_on","contradicts","supersedes"]}),
                    ),
                    ("cap", json!({"type":"integer","minimum":120,"maximum":500})),
                ],
                &[],
            ),
        ),
        spec(
            "memory_feedback",
            "Mark a visible memory helpful or wrong. Votes change ranking for everyone who sees it; a wrong vote from the owner marks it stale.",
            Post,
            "/feedback",
            object_schema(
                &[
                    ("memoryId", string_schema()),
                    ("vote", json!({"type":"string","enum":["helpful","wrong"]})),
                ],
                &["memoryId", "vote"],
            ),
        ),
        spec(
            "memory_project_memories",
            "List live project memories.",
            Get,
            "/projects/{projectId}/memories",
            project_schema(),
        ),
        spec(
            "memory_project_members",
            "List the project roster, visible to all members.",
            Get,
            "/projects/{projectId}/members",
            project_schema(),
        ),
        spec(
            "memory_delete",
            "Forget a memory you own (same as memory_forget).",
            Post,
            "/delete",
            object_schema(
                &[("id", string_schema()), ("memoryId", string_schema())],
                &["id"],
            ),
        ),
    ]
}

/// The per-kind keys of `fields`; anything else is rejected.
const FIELDS_HELP: &str = "Per-kind keys. decision: context, alternatives[], consequences. rule: exceptions. gotcha: symptom, cause, fix. procedure: steps[] (ordered), when. fact: source. preference: example. note: none.";

fn query_schema() -> Value {
    object_schema(
        &[
            ("query", string_schema()),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("sessionId", string_schema()),
            ("context", json!({"type": "object"})),
        ],
        &["query"],
    )
}

fn card_query_schema() -> Value {
    object_schema(
        &[
            ("query", string_schema()),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("sessionId", string_schema()),
            ("context", json!({"type": "object"})),
            (
                "expanded",
                json!({"type": "boolean", "description": "When true, return useful recall fields instead of the short card. Omitted lifecycle, internal ids, and duplicated content stay on memory_get."}),
            ),
        ],
        &["query"],
    )
}

fn continuity_schema() -> Value {
    object_schema(
        &[
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("sessionId", string_schema()),
            ("context", json!({"type": "object"})),
        ],
        &[],
    )
}

fn session_schema() -> Value {
    object_schema(&[("sessionId", string_schema())], &["sessionId"])
}

fn object_schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties: Map<String, Value> = properties
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": true
    })
}

fn string_schema() -> Value {
    json!({ "type": "string" })
}

fn array_schema() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_filled_encoded_and_removed_from_the_body() {
        let mut body = json!({"projectId": "p/1 x", "branchId": "b-1", "reason": "done"})
            .as_object()
            .unwrap()
            .clone();
        let path = fill_path("/projects/{projectId}/branches/{branchId}/close", &mut body).unwrap();
        assert_eq!(path, "/projects/p%2F1%20x/branches/b-1/close");
        assert_eq!(body.len(), 1);
        assert!(fill_path("/projects/{projectId}", &mut body).is_err());
    }

    #[test]
    fn dot_only_path_segments_are_rejected() {
        for value in [".", ".."] {
            let mut body = json!({"projectId": value}).as_object().unwrap().clone();
            let error = fill_path("/projects/{projectId}/members", &mut body).unwrap_err();
            assert!(matches!(error, Error::Input(_)), "{value}");
        }
        let mut body = json!({"projectId": "a.b..c"}).as_object().unwrap().clone();
        assert_eq!(
            fill_path("/projects/{projectId}", &mut body).unwrap(),
            "/projects/a.b..c"
        );
    }

    #[test]
    fn tool_names_are_unique_and_carry_no_account_token() {
        let mut names: Vec<_> = tools().iter().map(|tool| tool.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), tools().len());
        for tool in tools() {
            assert!(
                !tool.definition().to_string().contains("ccountToken"),
                "{}",
                tool.name
            );
        }
    }
}
