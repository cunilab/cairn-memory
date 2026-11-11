//! Minimal MCP stdio compatibility transport for the file-backed Cairn core.
//!
//! The transport intentionally implements the handshake-era MCP wire shape:
//! newline-delimited JSON-RPC messages on stdin/stdout, `initialize`,
//! `notifications/initialized`, `tools/list`, `tools/call`, and `ping`.
//! Application state remains in `cairn-core`; this module only translates MCP
//! requests into core operations and returns text plus structured results.

use cairn_core::{
    ExportBundle, LifecycleInput, MemoryKind, MemoryStore, RecallFilter, StoreError, WorkContext,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

const TOOL_NAMES: &[&str] = &[
    "memory_save",
    "memory_recall",
    "memory_consolidate",
    "memory_smart_search",
    "memory_why_changed",
    "memory_sessions",
    "memory_diagnose",
    "memory_lesson_save",
    "memory_reflect",
    "memory_session_start",
    "memory_observe",
    "memory_session_end",
    "memory_search",
    "memory_context",
    "memory_resume",
    "memory_unresolved",
    "memory_forget",
    "memory_export",
    "memory_import",
    "memory_correct",
    "memory_account_bootstrap",
    "memory_project_register",
    "memory_promote",
    "memory_project_memories",
    "memory_project_freshness",
    "memory_account_register",
    "memory_project_invite",
    "memory_project_share",
    "memory_project_shared",
    "memory_project_revoke",
];

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

pub struct McpServer {
    store: MemoryStore,
    initialized: bool,
}

impl McpServer {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self {
            store: MemoryStore::open(path)?,
            initialized: false,
        })
    }

    /// Run the standalone newline-delimited JSON-RPC stdio transport.
    pub fn run_stdio(mut self) -> Result<(), StoreError> {
        let stdin = io::stdin();
        let mut stdout = io::BufWriter::new(io::stdout().lock());

        for line in stdin.lock().lines() {
            let line = line.map_err(StoreError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(&line) {
                writeln!(stdout, "{response}").map_err(StoreError::Io)?;
                stdout.flush().map_err(StoreError::Io)?;
            }
        }
        Ok(())
    }

    /// Handle one JSON-RPC message. Notifications intentionally return no
    /// response, as required by JSON-RPC and the MCP stdio transport.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let parsed: Result<JsonRpcRequest, _> = serde_json::from_str(line);
        let request = match parsed {
            Ok(request) => request,
            Err(error) => return Some(rpc_error(Value::Null, -32700, error.to_string())),
        };

        let id = request.id.clone();
        let is_notification = id.is_none();
        let response = self.handle_request(request);
        if is_notification {
            None
        } else {
            Some(response)
        }
    }

    fn handle_request(&mut self, request: JsonRpcRequest) -> String {
        let id = request.id.unwrap_or(Value::Null);
        if request.jsonrpc.as_deref() != Some("2.0") {
            return rpc_error(id, -32600, "request must use JSON-RPC version 2.0");
        }

        match request.method.as_str() {
            "initialize" => self.initialize(id, request.params),
            "notifications/initialized" | "notifications/cancelled" => String::new(),
            "ping" => rpc_result(id, json!({})),
            "tools/list" => {
                if !self.initialized {
                    rpc_error(id, -32002, "MCP session is not initialized")
                } else {
                    rpc_result(id, json!({ "tools": tool_definitions() }))
                }
            }
            "tools/call" => {
                if !self.initialized {
                    rpc_error(id, -32002, "MCP session is not initialized")
                } else {
                    self.call_tool(id, request.params)
                }
            }
            _ => rpc_error(id, -32601, format!("method not found: {}", request.method)),
        }
    }

    fn initialize(&mut self, id: Value, params: Option<Value>) -> String {
        self.initialized = true;
        let requested_version = params
            .as_ref()
            .and_then(|params| params.get("protocolVersion"))
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);

        let result = json!({
            "protocolVersion": if requested_version == PROTOCOL_VERSION {
                requested_version
            } else {
                PROTOCOL_VERSION
            },
            "capabilities": {
                "tools": {"listChanged": false}
            },
            "serverInfo": {
                "name": "cairn-memory",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": "Cairn manages bounded personal memory with evidence-backed recall."
        });
        rpc_result(id, result)
    }

    fn call_tool(&mut self, id: Value, params: Option<Value>) -> String {
        let params = match params {
            Some(Value::Object(params)) => params,
            _ => return rpc_error(id, -32602, "tools/call params must be an object"),
        };
        let name = match params.get("name").and_then(Value::as_str) {
            Some(name) => name,
            None => return rpc_error(id, -32602, "tools/call requires a tool name"),
        };
        if !TOOL_NAMES.contains(&name) {
            return rpc_error(id, -32602, format!("unknown tool: {name}"));
        }

        let arguments = match params.get("arguments") {
            None => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => return rpc_error(id, -32602, "tool arguments must be an object"),
        };

        match self.execute_tool(name, &arguments) {
            Ok(value) => rpc_result(id, tool_success(value)),
            Err(error) => rpc_result(id, tool_failure(error)),
        }
    }

    fn execute_tool(&mut self, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
        match name {
            "memory_save" | "memory_lesson_save" => {
                let content = required_string(args, "content")?;
                let mut concepts = string_array(args, "concepts")?;
                if name == "memory_lesson_save" && !concepts.iter().any(|item| item == "lesson") {
                    concepts.push("lesson".to_owned());
                }
                let kind = if name == "memory_lesson_save" {
                    MemoryKind::Lesson
                } else {
                    MemoryKind::Explicit
                };
                let memory = self
                    .store
                    .remember_with_kind(&content, concepts, context(args)?, lifecycle(args)?, kind)
                    .map_err(store_message)?;
                serde_json::to_value(memory).map_err(|error| error.to_string())
            }
            "memory_recall" | "memory_context" | "memory_reflect" => {
                let query = required_string(args, "query")?;
                let limit = limit(args)?;
                let pack = self
                    .store
                    .context_with_filter(&query, limit, &recall_filter(args)?)
                    .map_err(store_message)?;
                if name == "memory_reflect" {
                    Ok(json!({
                        "query": pack.query,
                        "results": pack.results,
                        "omitted_count": pack.omitted_count,
                        "mode": "retrieval-only-layer-1",
                        "note": "No model-generated reflection is performed by the local Layer 1 adapter."
                    }))
                } else {
                    serde_json::to_value(pack).map_err(|error| error.to_string())
                }
            }
            "memory_resume" => {
                let pack = self
                    .store
                    .resume_context(limit(args)?, &recall_filter(args)?)
                    .map_err(store_message)?;
                serde_json::to_value(pack).map_err(|error| error.to_string())
            }
            "memory_unresolved" => {
                let pack = self
                    .store
                    .unresolved_context(limit(args)?, &recall_filter(args)?)
                    .map_err(store_message)?;
                serde_json::to_value(pack).map_err(|error| error.to_string())
            }
            "memory_smart_search" | "memory_search" => {
                let query = required_string(args, "query")?;
                let results = self
                    .store
                    .search_with_filter(&query, limit(args)?, &recall_filter(args)?)
                    .map_err(store_message)?;
                serde_json::to_value(results).map_err(|error| error.to_string())
            }
            "memory_why_changed" => {
                let query = required_string(args, "query")?;
                let pack = self
                    .store
                    .why_changed_context(&query, limit(args)?, &recall_filter(args)?)
                    .map_err(store_message)?;
                serde_json::to_value(pack).map_err(|error| error.to_string())
            }
            "memory_consolidate" | "memory_session_end" => {
                let session_id = required_string_alias(args, "sessionId", "session_id")?;
                let memory = self.store.end_session(&session_id).map_err(store_message)?;
                serde_json::to_value(memory).map_err(|error| error.to_string())
            }
            "memory_session_start" => {
                let session_id = required_string_alias(args, "sessionId", "session_id")?;
                let session = self
                    .store
                    .start_session(session_id, context(args)?)
                    .map_err(store_message)?;
                serde_json::to_value(session).map_err(|error| error.to_string())
            }
            "memory_observe" => {
                let event_id = required_string_alias(args, "eventId", "event_id")?;
                let session_id = required_string_alias(args, "sessionId", "session_id")?;
                let content = required_string(args, "content")?;
                let role = required_string(args, "role")?;
                let source = required_string(args, "source")?;
                let importance = optional_string(args, "importance")?;
                let result = self
                    .store
                    .capture_observation(cairn_core::ObservationInput {
                        event_id,
                        session_id,
                        content,
                        role,
                        source,
                        importance,
                        lifecycle: lifecycle(args)?,
                        context: context(args)?,
                    })
                    .map_err(store_message)?;
                serde_json::to_value(result).map_err(|error| error.to_string())
            }
            "memory_sessions" => {
                let sessions: Vec<_> = self.store.sessions().cloned().collect();
                Ok(json!({ "sessions": sessions }))
            }
            "memory_diagnose" => Ok(json!({
                "ok": true,
                "service": "cairn-memory",
                "storage": "file-backed-layer-3-prototype",
                "semantic_search": "disabled",
                "sessions": self.store.sessions().count(),
                "observations": self.store.observations().count(),
                "lifecycle": self.store.lifecycle_summary()
            })),
            "memory_forget" => {
                let id = args
                    .get("id")
                    .or_else(|| args.get("targetId"))
                    .or_else(|| args.get("target_id"))
                    .or_else(|| args.get("memoryId"))
                    .or_else(|| args.get("memory_id"))
                    .or_else(|| args.get("eventId"))
                    .or_else(|| args.get("event_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "id is required".to_owned())?;
                self.store.forget(id).map_err(store_message)?;
                Ok(json!({ "forgotten": true, "id": id }))
            }
            "memory_export" => {
                serde_json::to_value(self.store.export()).map_err(|error| error.to_string())
            }
            "memory_import" => {
                let bundle = args
                    .get("bundle")
                    .ok_or_else(|| "bundle is required".to_owned())
                    .and_then(|value| {
                        serde_json::from_value::<ExportBundle>(value.clone())
                            .map_err(|error| format!("invalid bundle: {error}"))
                    })?;
                let result = self.store.import_bundle(bundle).map_err(store_message)?;
                serde_json::to_value(result).map_err(|error| error.to_string())
            }
            "memory_correct" => {
                let target_id = args
                    .get("targetId")
                    .or_else(|| args.get("target_id"))
                    .or_else(|| args.get("memoryId"))
                    .or_else(|| args.get("memory_id"))
                    .or_else(|| args.get("eventId"))
                    .or_else(|| args.get("event_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "targetId is required".to_owned())?;
                let memory = self
                    .store
                    .correct(
                        target_id,
                        &required_string(args, "content")?,
                        string_array(args, "concepts")?,
                        context(args)?,
                        lifecycle(args)?,
                    )
                    .map_err(store_message)?;
                serde_json::to_value(memory).map_err(|error| error.to_string())
            }
            "memory_account_bootstrap" => {
                let email = required_string(args, "email")?;
                let password = required_string(args, "password")?;
                let session = self
                    .store
                    .bootstrap_account(&email, &password)
                    .map_err(store_message)?;
                serde_json::to_value(session).map_err(|error| error.to_string())
            }
            "memory_project_register" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let repository = required_string(args, "repository")?;
                let name = required_string(args, "name")?;
                let default_branch =
                    optional_string_alias(args, "defaultBranch", "default_branch")?;
                let project = self
                    .store
                    .register_project(&account_token, &repository, &name, default_branch)
                    .map_err(store_message)?;
                serde_json::to_value(project).map_err(|error| error.to_string())
            }
            "memory_promote" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let memory_id = required_string_alias(args, "memoryId", "memory_id")?;
                let reason = required_string(args, "reason")?;
                let memory = self
                    .store
                    .promote_memory(&account_token, &project_id, &memory_id, &reason)
                    .map_err(store_message)?;
                serde_json::to_value(memory).map_err(|error| error.to_string())
            }
            "memory_project_memories" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let memories = self
                    .store
                    .project_memories(&account_token, &project_id)
                    .map_err(store_message)?;
                serde_json::to_value(memories).map_err(|error| error.to_string())
            }
            "memory_project_freshness" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let events = self
                    .store
                    .freshness_history(&account_token, &project_id)
                    .map_err(store_message)?;
                serde_json::to_value(events).map_err(|error| error.to_string())
            }
            "memory_account_register" => {
                let email = required_string(args, "email")?;
                let password = required_string(args, "password")?;
                let session = self
                    .store
                    .register_account(&email, &password)
                    .map_err(store_message)?;
                serde_json::to_value(session).map_err(|error| error.to_string())
            }
            "memory_project_invite" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let email = required_string(args, "email")?;
                let membership = self
                    .store
                    .invite_project_member(&account_token, &project_id, &email)
                    .map_err(store_message)?;
                serde_json::to_value(membership).map_err(|error| error.to_string())
            }
            "memory_project_share" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let memory_id = required_string_alias(args, "memoryId", "memory_id")?;
                let email = required_string(args, "email")?;
                let share = self
                    .store
                    .share_project_memory(&account_token, &project_id, &memory_id, &email)
                    .map_err(store_message)?;
                serde_json::to_value(share).map_err(|error| error.to_string())
            }
            "memory_project_shared" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let memories = self
                    .store
                    .shared_project_memories(&account_token, &project_id)
                    .map_err(store_message)?;
                serde_json::to_value(memories).map_err(|error| error.to_string())
            }
            "memory_project_revoke" => {
                let account_token = required_string_alias(args, "accountToken", "account_token")?;
                let project_id = required_string_alias(args, "projectId", "project_id")?;
                let memory_id = required_string_alias(args, "memoryId", "memory_id")?;
                let email = required_string(args, "email")?;
                let share = self
                    .store
                    .revoke_project_share(&account_token, &project_id, &memory_id, &email)
                    .map_err(store_message)?;
                serde_json::to_value(share).map_err(|error| error.to_string())
            }
            _ => Err(format!("unsupported tool: {name}")),
        }
    }
}

pub fn run_stdio(path: impl AsRef<Path>) -> Result<(), StoreError> {
    McpServer::open(path)?.run_stdio()
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "memory_save",
            "Save an explicit durable memory.",
            object_schema(
                &[
                    ("content", string_schema()),
                    ("concepts", array_schema()),
                    ("lifecycle", lifecycle_schema()),
                ],
                &["content"],
            ),
        ),
        tool(
            "memory_recall",
            "Recall a bounded context pack for a query.",
            query_schema(),
        ),
        tool(
            "memory_consolidate",
            "Consolidate a captured session into an evidence-backed memory.",
            session_schema(),
        ),
        tool(
            "memory_smart_search",
            "Search durable memories using the available local retrieval index.",
            query_schema(),
        ),
        tool(
            "memory_why_changed",
            "Recover decisions, reasons, evidence, relationships, and stale or contradictory signals.",
            query_schema(),
        ),
        tool(
            "memory_sessions",
            "List sessions known to the local Cairn store.",
            object_schema(&[], &[]),
        ),
        tool(
            "memory_diagnose",
            "Report local storage and capture diagnostics.",
            object_schema(&[], &[]),
        ),
        tool(
            "memory_lesson_save",
            "Save an explicit lesson as durable memory.",
            object_schema(
                &[
                    ("content", string_schema()),
                    ("concepts", array_schema()),
                    ("lifecycle", lifecycle_schema()),
                ],
                &["content"],
            ),
        ),
        tool(
            "memory_reflect",
            "Return retrieval candidates for reflection without invoking a model.",
            query_schema(),
        ),
        tool(
            "memory_session_start",
            "Start or resume a capture session.",
            session_schema(),
        ),
        tool(
            "memory_observe",
            "Capture one bounded, replay-safe observation.",
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
        tool(
            "memory_session_end",
            "Finalize a capture session and create its summary memory.",
            session_schema(),
        ),
        tool(
            "memory_search",
            "Search durable memories by keyword.",
            query_schema(),
        ),
        tool(
            "memory_context",
            "Assemble a bounded context pack for an agent.",
            query_schema(),
        ),
        tool(
            "memory_resume",
            "Recover recent session summaries for the current work context.",
            continuity_schema(),
        ),
        tool(
            "memory_unresolved",
            "Recover recent session summaries with unresolved work.",
            continuity_schema(),
        ),
        tool(
            "memory_forget",
            "Forget an observation or derived memory and suppress future recall.",
            object_schema(
                &[
                    ("id", string_schema()),
                    ("eventId", string_schema()),
                    ("memoryId", string_schema()),
                ],
                &[],
            ),
        ),
        tool(
            "memory_export",
            "Export the local memory store as a versioned portable bundle.",
            object_schema(&[], &[]),
        ),
        tool(
            "memory_import",
            "Import a versioned portable memory bundle idempotently.",
            object_schema(&[("bundle", json!({"type": "object"}))], &["bundle"]),
        ),
        tool(
            "memory_correct",
            "Create a corrected durable memory while preserving the superseded item and trail.",
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
        tool(
            "memory_account_bootstrap",
            "Create the first file-backed account and return its session token.",
            object_schema(
                &[("email", string_schema()), ("password", string_schema())],
                &["email", "password"],
            ),
        ),
        tool(
            "memory_project_register",
            "Register a repository as a personal project. Requires the account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("repository", string_schema()),
                    ("name", string_schema()),
                    ("defaultBranch", string_schema()),
                ],
                &["accountToken", "repository", "name"],
            ),
        ),
        tool(
            "memory_promote",
            "Promote a repository-matched memory into project scope. Requires the account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                    ("memoryId", string_schema()),
                    ("reason", string_schema()),
                ],
                &["accountToken", "projectId", "memoryId", "reason"],
            ),
        ),
        tool(
            "memory_project_memories",
            "List live project memories. Requires the account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                ],
                &["accountToken", "projectId"],
            ),
        ),
        tool(
            "memory_project_freshness",
            "Read project freshness history. Requires the account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                ],
                &["accountToken", "projectId"],
            ),
        ),
        tool(
            "memory_account_register",
            "Register an account after bootstrap and return its session token.",
            object_schema(
                &[("email", string_schema()), ("password", string_schema())],
                &["email", "password"],
            ),
        ),
        tool(
            "memory_project_invite",
            "Invite an existing account to a project. Requires the owner account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                    ("email", string_schema()),
                ],
                &["accountToken", "projectId", "email"],
            ),
        ),
        tool(
            "memory_project_share",
            "Share one promoted project memory with an invited member. Requires the owner account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                    ("memoryId", string_schema()),
                    ("email", string_schema()),
                ],
                &["accountToken", "projectId", "memoryId", "email"],
            ),
        ),
        tool(
            "memory_project_shared",
            "List memories shared with the authenticated member for one project.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                ],
                &["accountToken", "projectId"],
            ),
        ),
        tool(
            "memory_project_revoke",
            "Revoke one member's share of a project memory. Requires the owner account token.",
            object_schema(
                &[
                    ("accountToken", string_schema()),
                    ("projectId", string_schema()),
                    ("memoryId", string_schema()),
                    ("email", string_schema()),
                ],
                &["accountToken", "projectId", "memoryId", "email"],
            ),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
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

fn required_string(args: &Map<String, Value>, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{key} is required"))
}

fn required_string_alias(
    args: &Map<String, Value>,
    preferred: &str,
    alias: &str,
) -> Result<String, String> {
    args.get(preferred)
        .or_else(|| args.get(alias))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{preferred} is required"))
}

fn optional_string(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| format!("{key} must be a string")),
    }
}

fn optional_string_alias(
    args: &Map<String, Value>,
    preferred: &str,
    alias: &str,
) -> Result<Option<String>, String> {
    match args.get(preferred).or_else(|| args.get(alias)) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            })
            .ok_or_else(|| format!("{preferred} must be a string")),
    }
}

fn string_array(args: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| format!("{key} must contain only strings"))
            })
            .collect(),
        Some(_) => Err(format!("{key} must be an array")),
    }
}

fn context(args: &Map<String, Value>) -> Result<WorkContext, String> {
    match args.get("context") {
        None | Some(Value::Null) => Ok(WorkContext::default()),
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| error.to_string()),
    }
}

fn recall_filter(args: &Map<String, Value>) -> Result<RecallFilter, String> {
    let session_id = args
        .get("sessionId")
        .or_else(|| args.get("session_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    let context = match args.get("context") {
        None | Some(Value::Null) => None,
        Some(value) => {
            Some(serde_json::from_value(value.clone()).map_err(|error| error.to_string())?)
        }
    };
    Ok(RecallFilter {
        session_id,
        context,
    })
}

fn lifecycle(args: &Map<String, Value>) -> Result<LifecycleInput, String> {
    match args.get("lifecycle") {
        None | Some(Value::Null) => Ok(LifecycleInput::default()),
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| error.to_string()),
    }
}

fn limit(args: &Map<String, Value>) -> Result<usize, String> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(5),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "limit must be a positive integer".to_owned()),
    }
}

fn store_message(error: StoreError) -> String {
    error.to_string()
}

fn tool_success(value: Value) -> Value {
    json!({
        "isError": false,
        "content": [{"type": "text", "text": text_for(&value)}],
        "structuredContent": value
    })
}

fn tool_failure(message: String) -> Value {
    json!({
        "isError": true,
        "content": [{"type": "text", "text": message}]
    })
}

fn text_for(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn rpc_result(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()}
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cairn-mcp-{name}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn request(id: u64, method: &str, params: Value) -> String {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })
        .to_string()
    }

    fn response(server: &mut McpServer, line: String) -> Value {
        serde_json::from_str(&server.handle_line(&line).expect("request response")).unwrap()
    }

    fn initialize(server: &mut McpServer) {
        let response = response(
            server,
            request(
                1,
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "test-client", "version": "0.1.0"}
                }),
            ),
        );
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(
            server
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
    }

    #[test]
    fn advertises_tools_and_runs_capture_to_forget() {
        let path = temp_store_path("flow");
        let mut server = McpServer::open(&path).unwrap();
        initialize(&mut server);

        let tools = response(&mut server, request(2, "tools/list", json!({})));
        let tools = tools["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "memory_observe"));
        assert!(tools.iter().any(|tool| tool["name"] == "memory_recall"));
        assert!(tools.iter().any(|tool| tool["name"] == "memory_resume"));

        let started = response(
            &mut server,
            request(
                3,
                "tools/call",
                json!({"name":"memory_session_start","arguments":{"sessionId":"mcp-session"}}),
            ),
        );
        assert_eq!(started["result"]["isError"], false);

        let captured = response(
            &mut server,
            request(
                4,
                "tools/call",
                json!({
                    "name":"memory_observe",
                    "arguments": {
                        "eventId":"mcp-event",
                        "sessionId":"mcp-session",
                        "content":"The MCP capture test passed.",
                        "role":"agent",
                        "source":"test",
                        "lifecycle":{"importance":90,"confidence":95}
                    }
                }),
            ),
        );
        assert_eq!(captured["result"]["structuredContent"]["accepted"], true);
        assert_eq!(
            captured["result"]["structuredContent"]["observation"]["lifecycle"]["importance"],
            90
        );

        let ended = response(
            &mut server,
            request(
                5,
                "tools/call",
                json!({"name":"memory_session_end","arguments":{"sessionId":"mcp-session"}}),
            ),
        );
        assert_eq!(
            ended["result"]["structuredContent"]["source_event_ids"][0],
            "mcp-event"
        );

        let resumed = response(
            &mut server,
            request(
                52,
                "tools/call",
                json!({"name":"memory_resume","arguments":{"limit":5}}),
            ),
        );
        assert_eq!(
            resumed["result"]["structuredContent"]["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let corrected = response(
            &mut server,
            request(
                51,
                "tools/call",
                json!({
                    "name":"memory_correct",
                    "arguments": {
                        "targetId": ended["result"]["structuredContent"]["memory_id"],
                        "content":"The MCP capture test passed and was corrected.",
                        "lifecycle":{"confidence":95}
                    }
                }),
            ),
        );
        assert_eq!(
            corrected["result"]["structuredContent"]["kind"],
            "correction"
        );

        let recalled = response(
            &mut server,
            request(
                6,
                "tools/call",
                json!({"name":"memory_recall","arguments":{"query":"MCP capture","limit":5}}),
            ),
        );
        assert_eq!(
            recalled["result"]["structuredContent"]["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let forgotten = response(
            &mut server,
            request(
                7,
                "tools/call",
                json!({"name":"memory_forget","arguments":{"eventId":"mcp-event"}}),
            ),
        );
        assert_eq!(forgotten["result"]["structuredContent"]["forgotten"], true);

        let recalled = response(
            &mut server,
            request(
                8,
                "tools/call",
                json!({"name":"memory_recall","arguments":{"query":"MCP capture","limit":5}}),
            ),
        );
        let results = recalled["result"]["structuredContent"]["results"]
            .as_array()
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0]["memory_id"],
            corrected["result"]["structuredContent"]["memory_id"]
        );

        let exported = response(
            &mut server,
            request(
                9,
                "tools/call",
                json!({"name":"memory_export","arguments":{}}),
            ),
        );
        let bundle = exported["result"]["structuredContent"].clone();
        assert_eq!(bundle["format"], "cairn-memory");
        let imported = response(
            &mut server,
            request(
                10,
                "tools/call",
                json!({"name":"memory_import","arguments":{"bundle":bundle}}),
            ),
        );
        assert_eq!(
            imported["result"]["structuredContent"]["imported_memories"],
            0
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scoped_recall_does_not_leak_worktree_memory() {
        let path = temp_store_path("scoped-recall");
        let mut server = McpServer::open(&path).unwrap();
        initialize(&mut server);

        let project = response(
            &mut server,
            request(
                2,
                "tools/call",
                json!({
                    "name":"memory_save",
                    "arguments": {
                        "content":"The project cache convention is documented.",
                        "context":{
                            "repository":"cairn-demo",
                            "files":["src/cache.rs"],
                            "symbols":["Cache::get"],
                            "commands":["cargo test cache"],
                            "tests":["cache_reads_are_consistent"],
                            "services":["cache"]
                        }
                    }
                }),
            ),
        );
        let branch = response(
            &mut server,
            request(
                3,
                "tools/call",
                json!({
                    "name":"memory_save",
                    "arguments": {
                        "content":"The Redis cache experiment is branch-local.",
                        "context": {
                            "repository":"cairn-demo",
                            "branch":"experiment/redis-cache",
                            "worktree":"wt-cache"
                        }
                    }
                }),
            ),
        );
        let recalled = response(
            &mut server,
            request(
                4,
                "tools/call",
                json!({
                    "name":"memory_recall",
                    "arguments": {
                        "query":"cache",
                        "limit":10,
                        "context": {
                            "repository":"cairn-demo",
                            "branch":"main",
                            "worktree":"wt-main"
                        }
                    }
                }),
            ),
        );
        let results = recalled["result"]["structuredContent"]["results"]
            .as_array()
            .unwrap();
        assert!(results.iter().any(|result| {
            result["memory_id"] == project["result"]["structuredContent"]["memory_id"]
        }));
        assert!(!results.iter().any(|result| {
            result["memory_id"] == branch["result"]["structuredContent"]["memory_id"]
        }));
        let project_result = results
            .iter()
            .find(|result| {
                result["memory_id"] == project["result"]["structuredContent"]["memory_id"]
            })
            .unwrap();
        assert_eq!(project_result["context"]["symbols"][0], "Cache::get");
        assert_eq!(project_result["context"]["commands"][0], "cargo test cache");
        assert_eq!(
            project_result["context"]["tests"][0],
            "cache_reads_are_consistent"
        );
        assert_eq!(project_result["context"]["services"][0], "cache");

        let why_changed = response(
            &mut server,
            request(
                5,
                "tools/call",
                json!({
                    "name":"memory_why_changed",
                    "arguments": {
                        "query":"cache",
                        "limit":10,
                        "context":{"repository":"cairn-demo"}
                    }
                }),
            ),
        );
        assert_eq!(
            why_changed["result"]["structuredContent"]["results"][0]["memory_id"],
            project["result"]["structuredContent"]["memory_id"]
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn project_tools_promote_and_then_correct() {
        let path = temp_store_path("project-flow");
        let mut server = McpServer::open(&path).unwrap();
        initialize(&mut server);

        let tools = response(&mut server, request(2, "tools/list", json!({})));
        let names = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        for name in [
            "memory_account_bootstrap",
            "memory_project_register",
            "memory_promote",
            "memory_project_memories",
            "memory_project_freshness",
        ] {
            assert!(names.contains(&name), "{name} missing from tools/list");
        }

        let missing_token = response(
            &mut server,
            request(
                3,
                "tools/call",
                json!({
                    "name":"memory_promote",
                    "arguments":{
                        "projectId":"project-missing",
                        "memoryId":"memory-missing",
                        "reason":"no token"
                    }
                }),
            ),
        );
        assert_eq!(missing_token["result"]["isError"], true);

        let bootstrap = response(
            &mut server,
            request(
                4,
                "tools/call",
                json!({
                    "name":"memory_account_bootstrap",
                    "arguments":{
                        "email":"owner@example.test",
                        "password":"correct horse battery staple"
                    }
                }),
            ),
        );
        assert_eq!(
            bootstrap["result"]["isError"], false,
            "{}",
            bootstrap["result"]["content"]
        );
        let token = bootstrap["result"]["structuredContent"]["token"]
            .as_str()
            .unwrap()
            .to_owned();

        let registered = response(
            &mut server,
            request(
                5,
                "tools/call",
                json!({
                    "name":"memory_project_register",
                    "arguments":{
                        "account_token": token,
                        "repository":"cairn-demo",
                        "name":"Cairn Demo",
                        "default_branch":"main"
                    }
                }),
            ),
        );
        assert_eq!(
            registered["result"]["isError"], false,
            "{}",
            registered["result"]["content"]
        );
        let project_id = registered["result"]["structuredContent"]["project_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let unknown_project = response(
            &mut server,
            request(
                6,
                "tools/call",
                json!({
                    "name":"memory_project_memories",
                    "arguments":{
                        "accountToken": token,
                        "projectId":"project-missing"
                    }
                }),
            ),
        );
        assert_eq!(unknown_project["result"]["isError"], true);

        let saved = response(
            &mut server,
            request(
                7,
                "tools/call",
                json!({
                    "name":"memory_save",
                    "arguments":{
                        "content":"Promote the cache decision because the regression passed.",
                        "context":{"repository":"cairn-demo","commit":"abc1234"}
                    }
                }),
            ),
        );
        assert_eq!(
            saved["result"]["isError"], false,
            "{}",
            saved["result"]["content"]
        );
        let memory_id = saved["result"]["structuredContent"]["memory_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let promoted = response(
            &mut server,
            request(
                8,
                "tools/call",
                json!({
                    "name":"memory_promote",
                    "arguments":{
                        "accountToken": token,
                        "project_id": project_id,
                        "memory_id": memory_id,
                        "reason":"Confirmed by the MCP project flow"
                    }
                }),
            ),
        );
        assert_eq!(
            promoted["result"]["isError"], false,
            "{}",
            promoted["result"]["content"]
        );
        let promoted_id = promoted["result"]["structuredContent"]["memory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(promoted["result"]["structuredContent"]["scope"], "project");

        let listed = response(
            &mut server,
            request(
                9,
                "tools/call",
                json!({
                    "name":"memory_project_memories",
                    "arguments":{"account_token": token, "project_id": project_id}
                }),
            ),
        );
        assert_eq!(
            listed["result"]["isError"], false,
            "{}",
            listed["result"]["content"]
        );
        let memories = listed["result"]["structuredContent"].as_array().unwrap();
        assert!(memories.iter().any(|memory| {
            memory["memory_id"].as_str() == Some(promoted_id.as_str())
                && memory["scope"] == "project"
        }));

        let freshness = response(
            &mut server,
            request(
                10,
                "tools/call",
                json!({
                    "name":"memory_project_freshness",
                    "arguments":{"accountToken": token, "projectId": project_id}
                }),
            ),
        );
        assert_eq!(
            freshness["result"]["isError"], false,
            "{}",
            freshness["result"]["content"]
        );
        let events = freshness["result"]["structuredContent"].as_array().unwrap();
        assert!(events.iter().any(|event| event["action"] == "promotion"));

        let corrected = response(
            &mut server,
            request(
                11,
                "tools/call",
                json!({
                    "name":"memory_correct",
                    "arguments":{
                        "targetId": promoted_id,
                        "content":"The cache decision stays project-scoped after correction."
                    }
                }),
            ),
        );
        assert_eq!(
            corrected["result"]["isError"], false,
            "{}",
            corrected["result"]["content"]
        );
        let correction_id = corrected["result"]["structuredContent"]["memory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            corrected["result"]["structuredContent"]["supersedes"],
            promoted_id
        );

        let listed_again = response(
            &mut server,
            request(
                12,
                "tools/call",
                json!({
                    "name":"memory_project_memories",
                    "arguments":{"accountToken": token, "projectId": project_id}
                }),
            ),
        );
        assert_eq!(
            listed_again["result"]["isError"], false,
            "{}",
            listed_again["result"]["content"]
        );
        let memories = listed_again["result"]["structuredContent"]
            .as_array()
            .unwrap();
        assert!(
            !memories
                .iter()
                .any(|memory| memory["memory_id"].as_str() == Some(promoted_id.as_str()))
        );
        assert!(memories.iter().any(|memory| {
            memory["memory_id"].as_str() == Some(correction_id.as_str())
                && memory["supersedes"].as_str() == Some(promoted_id.as_str())
        }));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn share_tools_invite_share_read_and_revoke() {
        let path = temp_store_path("share-flow");
        let mut server = McpServer::open(&path).unwrap();
        initialize(&mut server);

        let tools = response(&mut server, request(2, "tools/list", json!({})));
        let names = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        for name in [
            "memory_account_register",
            "memory_project_invite",
            "memory_project_share",
            "memory_project_shared",
            "memory_project_revoke",
        ] {
            assert!(names.contains(&name), "{name} missing from tools/list");
        }

        let missing_token = response(
            &mut server,
            request(
                3,
                "tools/call",
                json!({
                    "name":"memory_project_invite",
                    "arguments":{"projectId":"project-missing","email":"member@example.test"}
                }),
            ),
        );
        assert_eq!(missing_token["result"]["isError"], true);

        let bootstrap = response(
            &mut server,
            request(
                4,
                "tools/call",
                json!({
                    "name":"memory_account_bootstrap",
                    "arguments":{"email":"owner@example.test","password":"correct horse battery staple"}
                }),
            ),
        );
        assert_eq!(bootstrap["result"]["isError"], false);
        let owner_token = bootstrap["result"]["structuredContent"]["token"]
            .as_str()
            .unwrap()
            .to_owned();

        let registered_member = response(
            &mut server,
            request(
                5,
                "tools/call",
                json!({
                    "name":"memory_account_register",
                    "arguments":{"email":"member@example.test","password":"correct horse battery staple"}
                }),
            ),
        );
        assert_eq!(
            registered_member["result"]["isError"], false,
            "{}",
            registered_member["result"]["content"]
        );
        let member_token = registered_member["result"]["structuredContent"]["token"]
            .as_str()
            .unwrap()
            .to_owned();

        let project = response(
            &mut server,
            request(
                6,
                "tools/call",
                json!({
                    "name":"memory_project_register",
                    "arguments":{
                        "accountToken": owner_token,
                        "repository":"cairn-demo",
                        "name":"Cairn Demo"
                    }
                }),
            ),
        );
        assert_eq!(project["result"]["isError"], false);
        let project_id = project["result"]["structuredContent"]["project_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let unknown_email = response(
            &mut server,
            request(
                7,
                "tools/call",
                json!({
                    "name":"memory_project_invite",
                    "arguments":{
                        "account_token": owner_token,
                        "project_id": project_id,
                        "email":"missing@example.test"
                    }
                }),
            ),
        );
        assert_eq!(unknown_email["result"]["isError"], true);

        response(
            &mut server,
            request(
                8,
                "tools/call",
                json!({
                    "name":"memory_session_start",
                    "arguments":{"sessionId":"private-session"}
                }),
            ),
        );
        response(
            &mut server,
            request(
                9,
                "tools/call",
                json!({
                    "name":"memory_observe",
                    "arguments":{
                        "eventId":"evt-private",
                        "sessionId":"private-session",
                        "content":"private observation stays on the owner account",
                        "role":"agent",
                        "source":"test"
                    }
                }),
            ),
        );
        let private_ended = response(
            &mut server,
            request(
                10,
                "tools/call",
                json!({"name":"memory_session_end","arguments":{"sessionId":"private-session"}}),
            ),
        );
        assert_eq!(private_ended["result"]["isError"], false);
        let private_id = private_ended["result"]["structuredContent"]["memory_id"]
            .as_str()
            .unwrap()
            .to_owned();

        response(
            &mut server,
            request(
                11,
                "tools/call",
                json!({
                    "name":"memory_session_start",
                    "arguments":{
                        "sessionId":"repo-session",
                        "context":{"repository":"cairn-demo","commit":"abc1234"}
                    }
                }),
            ),
        );
        response(
            &mut server,
            request(
                12,
                "tools/call",
                json!({
                    "name":"memory_observe",
                    "arguments":{
                        "eventId":"evt-repo",
                        "sessionId":"repo-session",
                        "content":"repository cache decision is ready for the project",
                        "role":"agent",
                        "source":"test"
                    }
                }),
            ),
        );
        let repo_ended = response(
            &mut server,
            request(
                13,
                "tools/call",
                json!({"name":"memory_session_end","arguments":{"sessionId":"repo-session"}}),
            ),
        );
        assert_eq!(repo_ended["result"]["isError"], false);
        let repo_id = repo_ended["result"]["structuredContent"]["memory_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let promoted = response(
            &mut server,
            request(
                14,
                "tools/call",
                json!({
                    "name":"memory_promote",
                    "arguments":{
                        "accountToken": owner_token,
                        "projectId": project_id,
                        "memoryId": repo_id,
                        "reason":"Share the promoted repository decision"
                    }
                }),
            ),
        );
        assert_eq!(
            promoted["result"]["isError"], false,
            "{}",
            promoted["result"]["content"]
        );
        let promoted_id = promoted["result"]["structuredContent"]["memory_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let invited = response(
            &mut server,
            request(
                15,
                "tools/call",
                json!({
                    "name":"memory_project_invite",
                    "arguments":{
                        "accountToken": owner_token,
                        "projectId": project_id,
                        "email":"member@example.test"
                    }
                }),
            ),
        );
        assert_eq!(invited["result"]["isError"], false);

        let member_share = response(
            &mut server,
            request(
                16,
                "tools/call",
                json!({
                    "name":"memory_project_share",
                    "arguments":{
                        "account_token": member_token,
                        "project_id": project_id,
                        "memory_id": promoted_id,
                        "email":"member@example.test"
                    }
                }),
            ),
        );
        assert_eq!(member_share["result"]["isError"], true);

        let shared = response(
            &mut server,
            request(
                17,
                "tools/call",
                json!({
                    "name":"memory_project_share",
                    "arguments":{
                        "accountToken": owner_token,
                        "projectId": project_id,
                        "memoryId": promoted_id,
                        "email":"member@example.test"
                    }
                }),
            ),
        );
        assert_eq!(shared["result"]["isError"], false);

        let member_read = response(
            &mut server,
            request(
                18,
                "tools/call",
                json!({
                    "name":"memory_project_shared",
                    "arguments":{"account_token": member_token, "project_id": project_id}
                }),
            ),
        );
        assert_eq!(member_read["result"]["isError"], false);
        let memories = member_read["result"]["structuredContent"]
            .as_array()
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0]["memory_id"], promoted_id);
        assert!(
            !memories
                .iter()
                .any(|memory| memory["memory_id"].as_str() == Some(private_id.as_str()))
        );

        let member_search = response(
            &mut server,
            request(
                19,
                "tools/call",
                json!({
                    "name":"memory_search",
                    "arguments":{
                        "query":"private observation",
                        "limit":5,
                        "context":{"repository":"cairn-demo"}
                    }
                }),
            ),
        );
        assert_eq!(member_search["result"]["isError"], false);
        let results = member_search["result"]["structuredContent"]
            .as_array()
            .unwrap();
        assert!(
            !results
                .iter()
                .any(|result| result["memory_id"].as_str() == Some(private_id.as_str()))
        );

        let revoked = response(
            &mut server,
            request(
                20,
                "tools/call",
                json!({
                    "name":"memory_project_revoke",
                    "arguments":{
                        "accountToken": owner_token,
                        "projectId": project_id,
                        "memoryId": promoted_id,
                        "email":"member@example.test"
                    }
                }),
            ),
        );
        assert_eq!(revoked["result"]["isError"], false);

        let after_revoke = response(
            &mut server,
            request(
                21,
                "tools/call",
                json!({
                    "name":"memory_project_shared",
                    "arguments":{"accountToken": member_token, "projectId": project_id}
                }),
            ),
        );
        assert_eq!(after_revoke["result"]["isError"], false);
        assert_eq!(
            after_revoke["result"]["structuredContent"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_and_failed_tool_calls_are_protocol_safe() {
        let path = temp_store_path("errors");
        let mut server = McpServer::open(&path).unwrap();
        initialize(&mut server);

        let error = response(
            &mut server,
            request(2, "tools/call", json!({"name": "memory_save"})),
        );
        assert_eq!(error["result"]["isError"], true);
        assert!(
            error["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("content")
        );

        let unknown = response(
            &mut server,
            request(3, "tools/call", json!({"name": "unknown"})),
        );
        assert_eq!(unknown["error"]["code"], -32602);

        let parse_error = server.handle_line("not-json").unwrap();
        let parse_error: Value = serde_json::from_str(&parse_error).unwrap();
        assert_eq!(parse_error["error"]["code"], -32700);
        let _ = std::fs::remove_file(path);
    }
}
