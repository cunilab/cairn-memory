//! Minimal MCP stdio transport that proxies each tool call to the Cairn REST API with the
//! configured device token.
//!
//! Wire shape: newline-delimited JSON-RPC on stdin/stdout, `initialize`,
//! `notifications/initialized`, `tools/list`, `tools/call`, and `ping`. The tool table and
//! its REST mapping live in [`crate::tools`]; this module only speaks the protocol.

use crate::hook::git;
use crate::{Client, Config, Error};
use crate::{present, tools};
use cairn_core::account::ClientKind;
use cairn_core::retrieve::brief::SESSION_BRIEF_MAX_BYTES;
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};
use std::time::Duration;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// A line longer than this is skipped with an error, so one message cannot fill memory.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const BRIEF_TIMEOUT: Duration = Duration::from_secs(1);

enum Line {
    Eof,
    Text(Vec<u8>),
    TooLong,
}

/// Reads up to the next newline, keeping at most `cap` bytes; the rest of an oversized line
/// is consumed and dropped.
fn read_line(reader: &mut impl BufRead, cap: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut too_long = false;
    let mut seen = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        seen = true;
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let chunk = &buffer[..newline.unwrap_or(buffer.len())];
        if line.len() + chunk.len() > cap {
            too_long = true;
        } else if !too_long {
            line.extend_from_slice(chunk);
        }
        let consumed = newline.map_or(buffer.len(), |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    Ok(match (seen, too_long) {
        (false, _) => Line::Eof,
        (true, true) => Line::TooLong,
        (true, false) => Line::Text(line),
    })
}

pub struct McpServer {
    client: Client,
    /// What `client` was built from; a 401 reloads the file and retries when it changed.
    config: Config,
    initialized: bool,
    session_id: Option<String>,
}

impl McpServer {
    pub fn new(config: Config) -> Self {
        Self {
            client: Client::identified(config.clone(), crate::REQUEST_TIMEOUT, "", ClientKind::Mcp),
            config,
            initialized: false,
            session_id: None,
        }
    }

    /// `cairn setup` rotates the token in `config.json` under a running session: on a 401, read
    /// the file again and, when it holds another credential, switch to it and ask once more.
    fn call_with_fresh_config(
        &mut self,
        tool: &tools::Tool,
        arguments: Map<String, Value>,
    ) -> Result<Value, Error> {
        let result = tools::call(&self.client, tool, arguments.clone());
        if !matches!(result, Err(Error::Status { status: 401, .. })) {
            return result;
        }
        let Some(fresh) = self
            .config
            .path()
            .and_then(|path| Config::load_from(path).ok())
            .filter(|fresh| *fresh != self.config)
        else {
            return result;
        };
        self.client =
            Client::identified(fresh.clone(), crate::REQUEST_TIMEOUT, "", ClientKind::Mcp);
        self.config = fresh;
        tools::call(&self.client, tool, arguments)
    }

    /// Run the standalone newline-delimited JSON-RPC stdio transport.
    pub fn run_stdio(self) -> Result<(), Error> {
        let stdin = io::stdin();
        self.serve(
            &mut stdin.lock(),
            &mut io::BufWriter::new(io::stdout().lock()),
            MAX_LINE_BYTES,
        )
    }

    /// A bad line gets an error response and the loop continues; only I/O failure ends it.
    fn serve(
        mut self,
        reader: &mut impl BufRead,
        writer: &mut impl Write,
        max_line: usize,
    ) -> Result<(), Error> {
        let io_error = |error: io::Error| Error::Request(format!("stdio failed: {error}"));
        loop {
            let response = match read_line(reader, max_line).map_err(io_error)? {
                Line::Eof => return Ok(()),
                Line::TooLong => Some(rpc_error(Value::Null, -32600, "request line is too long")),
                Line::Text(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(line) if line.trim().is_empty() => None,
                    Ok(line) => self.handle_line(line),
                    Err(_) => Some(rpc_error(Value::Null, -32700, "request is not valid UTF-8")),
                },
            };
            if let Some(response) = response {
                writeln!(writer, "{response}").map_err(io_error)?;
                writer.flush().map_err(io_error)?;
            }
        }
    }

    /// Handle one JSON-RPC message. A notification (no `id` member) gets no response and
    /// runs nothing; `"id": null` is still a request and gets one.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let message = match serde_json::from_str::<Value>(line) {
            Ok(message) => message,
            Err(error) => return Some(rpc_error(Value::Null, -32700, error.to_string())),
        };
        let Value::Object(mut message) = message else {
            return Some(rpc_error(
                Value::Null,
                -32600,
                "request must be a JSON object",
            ));
        };
        let id = message.remove("id");
        if !matches!(
            &id,
            None | Some(Value::Null | Value::String(_) | Value::Number(_))
        ) {
            return Some(rpc_error(
                Value::Null,
                -32600,
                "request id must be a string, number or null",
            ));
        }
        let Some(method) = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Some(rpc_error(Value::Null, -32600, "request has no method"));
        };
        let id = id?;
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(rpc_error(
                id,
                -32600,
                "request must use JSON-RPC version 2.0",
            ));
        }
        Some(self.handle_request(id, &method, message.remove("params")))
    }

    fn handle_request(&mut self, id: Value, method: &str, params: Option<Value>) -> String {
        match method {
            "initialize" => self.initialize(id, params),
            "ping" => rpc_result(id, json!({})),
            "tools/list" | "tools/call" if !self.initialized => {
                rpc_error(id, -32002, "MCP session is not initialized")
            }
            "tools/list" => rpc_result(id, tools::tools_list()),
            "tools/call" => self.call_tool(id, params),
            _ => rpc_error(id, -32601, format!("method not found: {method}")),
        }
    }

    fn checkout_brief(&self, session_id: Option<&str>) -> Option<String> {
        let cwd = std::env::current_dir().ok()?;
        let top = git::top_level(&cwd);
        if git::ignored(&cwd, top.as_deref()) {
            return None;
        }
        let body = json!({
            "session_id": session_id,
            "context": git::work_context(&cwd, top.as_deref()),
            "recent_files": git::recent_files(&cwd),
            "extended_brief": true,
        });
        let client = Client::identified(self.config.clone(), BRIEF_TIMEOUT, "", ClientKind::Mcp);
        let reply = client.post("/cairn-memory/session/brief", &body).ok()?;
        let text = reply["brief"].as_str()?;
        (!text.is_empty() && text.len() <= SESSION_BRIEF_MAX_BYTES).then(|| text.to_owned())
    }

    fn initialize(&mut self, id: Value, params: Option<Value>) -> String {
        self.initialized = true;
        let requested_version = params
            .as_ref()
            .and_then(|params| params.get("protocolVersion"))
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);

        self.session_id = params
            .as_ref()
            .and_then(|params| params.pointer("/_meta/sessionId"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty() && id.len() <= 512 && !id.contains('\0'))
            .map(str::to_owned);
        let mut instructions = tools::instructions().to_owned();
        if let Some(brief) = self.checkout_brief(self.session_id.as_deref()) {
            instructions.push_str("\n\n");
            instructions.push_str(&brief);
        }

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
                "title": "Cairn",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": instructions
        });
        rpc_result(id, result)
    }

    fn call_tool(&mut self, id: Value, params: Option<Value>) -> String {
        let params = match params {
            Some(Value::Object(params)) => params,
            _ => return rpc_error(id, -32602, "tools/call params must be an object"),
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return rpc_error(id, -32602, "tools/call requires a tool name");
        };
        let Some(tool) = tools::find_by_name(name) else {
            return rpc_error(id, -32602, format!("unknown tool: {name}"));
        };
        let mut arguments = match params.get("arguments") {
            None => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => return rpc_error(id, -32602, "tool arguments must be an object"),
        };
        if let Some(session) = &self.session_id
            && matches!(name, "memory_get" | "memory_feedback" | "memory_save")
        {
            arguments.insert("session_id".to_owned(), json!(session));
        }
        match self.call_with_fresh_config(tool, arguments) {
            Ok(value) => rpc_result(id, present::success(&value)),
            Err(error) => rpc_result(id, present::failure(&error.to_string())),
        }
    }
}

pub fn run_stdio(config: Config) -> Result<(), Error> {
    McpServer::new(config).run_stdio()
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

    fn server() -> McpServer {
        // Nothing listens here, so initialize keeps the static instructions.
        McpServer::new(Config::new("http://127.0.0.1:1", "test-token").unwrap())
    }

    fn call(server: &mut McpServer, id: u64, method: &str, params: Value) -> Value {
        let line =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        serde_json::from_str(&server.handle_line(&line).expect("response")).unwrap()
    }

    #[test]
    fn handshake_gates_tools_and_lists_the_agent_surface() {
        let mut server = server();
        let early = call(&mut server, 1, "tools/list", json!({}));
        assert_eq!(early["error"]["code"], -32002);

        let init = call(
            &mut server,
            2,
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION}),
        );
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(init["result"]["serverInfo"]["name"], "cairn-memory");
        assert_eq!(init["result"]["serverInfo"]["title"], "Cairn");
        let instructions = init["result"]["instructions"].as_str().unwrap();
        assert_eq!(instructions, tools::instructions());
        assert!(instructions.starts_with(cairn_core::instructions::DEFAULT_SESSION_INSTRUCTION));
        assert!(instructions.len() <= 700);
        assert!(
            server
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );

        let listed = call(&mut server, 3, "tools/list", json!({}));
        let names: Vec<_> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "memory_search",
                "memory_get",
                "memory_save",
                "memory_feedback",
                "memory_forget",
                "skill_search",
                "skill_get",
                "skill_save",
                "skill_feedback"
            ]
        );
    }

    #[test]
    fn malformed_and_failed_tool_calls_are_protocol_safe() {
        let mut server = server();
        call(&mut server, 1, "initialize", json!({}));

        // memory_save has no top-level required, so a missing field is the server's
        // error. memory_get still rejects missing ids locally, before any request.
        let missing = call(&mut server, 2, "tools/call", json!({"name": "memory_get"}));
        assert_eq!(missing["result"]["isError"], true);
        assert_eq!(missing["result"]["content"][0]["text"], "ids is required");

        let unknown = call(&mut server, 3, "tools/call", json!({"name": "unknown"}));
        assert_eq!(unknown["error"]["code"], -32602);
        // CLI verbs are not MCP tool names.
        for verb in ["search", "get", "save", "feedback", "forget"] {
            let reply = call(&mut server, 3, "tools/call", json!({"name": verb}));
            assert_eq!(reply["error"]["code"], -32602, "{verb}");
        }

        let unreachable = call(
            &mut server,
            4,
            "tools/call",
            json!({"name": "memory_search", "arguments": {"query": "x"}}),
        );
        assert_eq!(unreachable["result"]["isError"], true);
        assert!(!unreachable.to_string().contains("test-token"));

        let parse_error: Value =
            serde_json::from_str(&server.handle_line("not-json").unwrap()).unwrap();
        assert_eq!(parse_error["error"]["code"], -32700);
    }

    #[test]
    fn a_notification_runs_nothing_and_a_null_id_is_still_a_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let mut server = McpServer::new(Config::new(&url, "test-token").unwrap());
        server.initialized = true;

        let side_effect = json!({"jsonrpc": "2.0", "method": "tools/call",
            "params": {"name": "memory_search", "arguments": {"query": "x"}}});
        assert!(server.handle_line(&side_effect.to_string()).is_none());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "a notification must not reach the server"
        );

        let reply = server
            .handle_line(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#)
            .expect("a null id is a request");
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            (reply["id"].clone(), reply["result"].clone()),
            (Value::Null, json!({}))
        );
    }

    /// Answers 200 to the token `good` and 401 to any other, until the process ends.
    fn serve_token(good: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            use std::io::Read;
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                // The whole request is read, so closing the socket never resets the client.
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let body_start = loop {
                    let read = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                    assert!(read > 0, "request ended before its head did");
                };
                let head = String::from_utf8_lossy(&request[..body_start]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map_or(0, |value| value.trim().parse().unwrap());
                while request.len() < body_start + length {
                    let read = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..read]);
                }
                let ok = head.contains(&format!("bearer {}", good.to_lowercase()));
                let (status, body) = if ok {
                    ("200 OK", r#"{"results":[]}"#)
                } else {
                    ("401 Unauthorized", r#"{"error":"bad token"}"#)
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_rotated_token_is_picked_up_after_a_401() {
        let url = serve_token("new-token");
        let dir = std::env::temp_dir().join(format!("cairn-mcp-rotate-{}", std::process::id()));
        let path = dir.join("config.json");
        Config::new(&url, "old-token").unwrap().save(&path).unwrap();
        let mut server = McpServer::new(Config::load_from(&path).unwrap());
        call(&mut server, 1, "initialize", json!({}));
        let search = |server: &mut McpServer, id| {
            call(
                server,
                id,
                "tools/call",
                json!({"name": "memory_search", "arguments": {"query": "x"}}),
            )
        };
        // The file still holds the rejected token: the 401 is reported, nothing to retry with.
        assert_eq!(search(&mut server, 2)["result"]["isError"], true);
        // `cairn setup` rotates it under the running session: the next call retries and works.
        Config::new(&url, "new-token").unwrap().save(&path).unwrap();
        let after = search(&mut server, 3);
        assert_ne!(after["result"]["isError"], true, "{after}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_json_that_is_not_a_request_is_invalid_request() {
        let mut server = server();
        for line in [
            "[]",
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#,
            "42",
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":1}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":7}"#,
            r#"{"jsonrpc":"2.0","id":{"a":1},"method":"ping"}"#,
            r#"{"id":1,"method":"ping"}"#,
        ] {
            let reply: Value =
                serde_json::from_str(&server.handle_line(line).expect(line)).unwrap();
            assert_eq!(reply["error"]["code"], -32600, "{line}");
        }
    }

    #[test]
    fn bad_lines_get_an_error_and_the_server_keeps_serving() {
        let ping = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut input = b"\xff\xfe not utf8\n".to_vec();
        input.extend_from_slice(format!("{}\n", "x".repeat(100)).as_bytes());
        input.extend_from_slice(ping.as_bytes());
        let mut output = Vec::new();
        server()
            .serve(&mut io::Cursor::new(input), &mut output, 64)
            .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let codes: Vec<_> = replies
            .iter()
            .map(|reply| reply["error"]["code"].clone())
            .collect();
        assert_eq!(codes, [json!(-32700), json!(-32600), Value::Null]);
        assert_eq!(replies[2]["result"], json!({}));
    }

    /// Compact JSON bytes of `memory_save`'s inputSchema before `items` was added.
    #[test]
    fn tool_count_and_memory_save_schema_budget() {
        let list = tools::tools_list();
        let tools = list["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 9);
        let save = tools
            .iter()
            .find(|tool| tool["name"] == "memory_save")
            .unwrap();
        let schema = save["inputSchema"].to_string();
        const BEFORE: usize = 532;
        assert!(
            schema.len() >= BEFORE,
            "schema shrank: {} < {BEFORE}",
            schema.len()
        );
        let grown = schema.len() - BEFORE;
        assert!(
            schema.contains("\"maxItems\":3"),
            "items schema missing: {schema}"
        );
        // Item 03 used at most 450 bytes. Item 09 adds at most 200 for aliases.
        assert!(grown <= 650, "schema grew by {grown}: {schema}");
        assert!(
            save["description"]
                .as_str()
                .unwrap()
                .contains("items: up to 3 notes in one call."),
            "{}",
            save["description"]
        );
    }
}
