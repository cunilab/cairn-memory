//! The agent tool surface. Each tool is one REST call; the `cairn` CLI and the MCP server
//! share this table, so a command and its tool cannot drift apart.
//!
//! Web-only actions (account and device management, member removal) have no tool: a device
//! token is refused there, so offering them would only invite retries.

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
    let path = fill_path(tool.path, &mut body)?;
    if tool.name == "memory_lesson_save" {
        body.insert("kind".to_owned(), json!("lesson"));
        let concepts = body.entry("concepts").or_insert_with(|| json!([]));
        if let Some(concepts) = concepts.as_array_mut()
            && !concepts.iter().any(|item| item == "lesson")
        {
            concepts.push(json!("lesson"));
        }
    }
    let route = format!("/cairn-memory{path}");
    let result = match tool.method {
        Method::Get => client.get(&route),
        Method::Post => client.post(&route, &Value::Object(body)),
    }?;
    if tool.name == "memory_reflect" {
        return Ok(json!({
            "query": result["query"],
            "results": result["results"],
            "omitted_count": result["omitted_count"],
            "mode": "retrieval-only",
            "note": "No model-generated reflection is performed."
        }));
    }
    Ok(result)
}

fn fill_path(template: &str, body: &mut Map<String, Value>) -> Result<String, Error> {
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

fn encode_byte(byte: u8) -> String {
    match byte {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
            char::from(byte).to_string()
        }
        _ => format!("%{byte:02X}"),
    }
}

fn build_tools() -> Vec<Tool> {
    let content_schema = || {
        object_schema(
            &[
                ("content", string_schema()),
                ("concepts", array_schema()),
                ("lifecycle", lifecycle_schema()),
            ],
            &["content"],
        )
    };
    let project_schema = || object_schema(&[("projectId", string_schema())], &["projectId"]);
    let project_email_schema = || {
        object_schema(
            &[("projectId", string_schema()), ("email", string_schema())],
            &["projectId", "email"],
        )
    };
    let share_schema = || {
        object_schema(
            &[
                ("projectId", string_schema()),
                ("memoryId", string_schema()),
                ("email", string_schema()),
            ],
            &["projectId", "memoryId", "email"],
        )
    };
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
            "Save an explicit durable memory.",
            Post,
            "/remember",
            content_schema(),
        ),
        spec(
            "memory_recall",
            "Recall a bounded context pack for a query.",
            Post,
            "/recall",
            query_schema(),
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
            "Search durable memories.",
            Post,
            "/smart-search",
            query_schema(),
        ),
        spec(
            "memory_why_changed",
            "Recover decisions, reasons, evidence, relationships, and stale or contradictory signals.",
            Post,
            "/why-changed",
            query_schema(),
        ),
        spec(
            "memory_diagnose",
            "Report memory lifecycle counts for the account.",
            Get,
            "/diagnose",
            object_schema(&[], &[]),
        ),
        spec(
            "memory_lesson_save",
            "Save an explicit lesson as durable memory.",
            Post,
            "/remember",
            content_schema(),
        ),
        spec(
            "memory_reflect",
            "Return retrieval candidates for reflection without invoking a model.",
            Post,
            "/context",
            query_schema(),
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
                    ("importance", string_schema()),
                    ("lifecycle", lifecycle_schema()),
                ],
                &["eventId", "sessionId", "content", "role", "source"],
            ),
        ),
        spec(
            "memory_session_end",
            "Finalize a capture session and create its summary memory.",
            Post,
            "/session/end",
            session_schema(),
        ),
        spec(
            "memory_search",
            "Search durable memories by keyword.",
            Post,
            "/search",
            query_schema(),
        ),
        spec(
            "memory_context",
            "Assemble a bounded context pack for an agent.",
            Post,
            "/context",
            query_schema(),
        ),
        spec(
            "memory_resume",
            "Recover recent session summaries for the current work context.",
            Post,
            "/resume",
            continuity_schema(),
        ),
        spec(
            "memory_unresolved",
            "Recover recent session summaries with unresolved work.",
            Post,
            "/unresolved",
            continuity_schema(),
        ),
        spec(
            "memory_forget",
            "Forget an observation or derived memory and suppress future recall.",
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
            "Export the account's memory as a versioned portable bundle.",
            Post,
            "/export",
            object_schema(&[], &[]),
        ),
        spec(
            "memory_import",
            "Import a versioned portable memory bundle idempotently.",
            Post,
            "/import",
            object_schema(&[("bundle", json!({"type": "object"}))], &["bundle"]),
        ),
        spec(
            "memory_correct",
            "Create a corrected durable memory while preserving the superseded item and trail.",
            Post,
            "/correct",
            object_schema(
                &[
                    ("targetId", string_schema()),
                    ("content", string_schema()),
                    ("concepts", array_schema()),
                    ("lifecycle", lifecycle_schema()),
                ],
                &["targetId", "content"],
            ),
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
            "Register a repository as a personal project.",
            Post,
            "/projects",
            object_schema(
                &[
                    ("repository", string_schema()),
                    ("name", string_schema()),
                    ("defaultBranch", string_schema()),
                ],
                &["repository", "name"],
            ),
        ),
        spec(
            "memory_promote",
            "Promote a repository-matched memory into project scope.",
            Post,
            "/promote",
            object_schema(
                &[
                    ("projectId", string_schema()),
                    ("memoryId", string_schema()),
                    ("reason", string_schema()),
                ],
                &["projectId", "memoryId", "reason"],
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
            "memory_project_freshness",
            "Read project freshness history.",
            Get,
            "/projects/{projectId}/freshness",
            project_schema(),
        ),
        spec(
            "memory_project_invite",
            "Invite an existing account to a project. Owner only.",
            Post,
            "/projects/{projectId}/members",
            project_email_schema(),
        ),
        spec(
            "memory_project_share",
            "Share one promoted project memory with an invited member. Owner only.",
            Post,
            "/projects/{projectId}/shares",
            share_schema(),
        ),
        spec(
            "memory_project_shared",
            "List memories shared with the account for one project.",
            Get,
            "/projects/{projectId}/shared",
            project_schema(),
        ),
        spec(
            "memory_project_revoke",
            "Revoke one member's share of a project memory. Owner only.",
            Post,
            "/projects/{projectId}/shares/{memoryId}/revoke",
            share_schema(),
        ),
        spec(
            "memory_project_team_create",
            "Create the project team space. Owner only.",
            Post,
            "/projects/{projectId}/team",
            object_schema(
                &[("projectId", string_schema()), ("name", string_schema())],
                &["projectId", "name"],
            ),
        ),
        spec(
            "memory_project_team",
            "List the project team and its members.",
            Get,
            "/projects/{projectId}/team",
            project_schema(),
        ),
        spec(
            "memory_project_team_add",
            "Add an existing project member to the team. Owner only.",
            Post,
            "/projects/{projectId}/team/members",
            project_email_schema(),
        ),
        spec(
            "memory_delete",
            "Hide a memory from normal recall while keeping it in storage and export. Owner only.",
            Post,
            "/delete",
            object_schema(
                &[("id", string_schema()), ("memoryId", string_schema())],
                &["id"],
            ),
        ),
    ]
}

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

fn lifecycle_schema() -> Value {
    object_schema(
        &[
            (
                "importance",
                json!({"type": "integer", "minimum": 0, "maximum": 100}),
            ),
            (
                "sensitivity",
                json!({"type": "string", "enum": ["public", "private", "sensitive", "secret"]}),
            ),
            (
                "confidence",
                json!({"type": "integer", "minimum": 0, "maximum": 100}),
            ),
            (
                "freshness",
                json!({"type": "integer", "minimum": 0, "maximum": 100}),
            ),
            (
                "retention",
                json!({"type": "string", "enum": ["temporary", "durable"]}),
            ),
            ("expiresAt", string_schema()),
        ],
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_filled_encoded_and_removed_from_the_body() {
        let mut body = json!({"projectId": "p/1 x", "memoryId": "m-1", "email": "a@b.test"})
            .as_object()
            .unwrap()
            .clone();
        let path = fill_path("/projects/{projectId}/shares/{memoryId}/revoke", &mut body).unwrap();
        assert_eq!(path, "/projects/p%2F1%20x/shares/m-1/revoke");
        assert_eq!(body.len(), 1);
        assert!(fill_path("/projects/{projectId}", &mut body).is_err());
    }

    #[test]
    fn dot_only_path_segments_are_rejected() {
        for value in [".", ".."] {
            let mut body = json!({"projectId": value}).as_object().unwrap().clone();
            let error = fill_path("/projects/{projectId}/team", &mut body).unwrap_err();
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
