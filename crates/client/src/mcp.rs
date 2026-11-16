//! Minimal MCP stdio transport that proxies each tool call to the Cairn REST API with the
//! configured device token.
//!
//! Wire shape: newline-delimited JSON-RPC on stdin/stdout, `initialize`,
//! `notifications/initialized`, `tools/list`, `tools/call`, and `ping`. The tool table and
//! its REST mapping live in [`crate::tools`]; this module only speaks the protocol.

use crate::tools;
use crate::{Client, Config, Error};
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// A line longer than this is skipped with an error, so one message cannot fill memory.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

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
    initialized: bool,
}

impl McpServer {
    pub fn new(config: Config) -> Self {
        Self {
            client: Client::new(config),
            initialized: false,
        }
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
            "tools/list" => {
                let definitions: Vec<Value> =
                    tools::tools().iter().map(tools::Tool::definition).collect();
                rpc_result(id, json!({ "tools": definitions }))
            }
            "tools/call" => self.call_tool(id, params),
            _ => rpc_error(id, -32601, format!("method not found: {method}")),
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
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return rpc_error(id, -32602, "tools/call requires a tool name");
        };
        let Some(tool) = tools::find(name) else {
            return rpc_error(id, -32602, format!("unknown tool: {name}"));
        };
        let arguments = match params.get("arguments") {
            None => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => return rpc_error(id, -32602, "tool arguments must be an object"),
        };

        match tools::call(&self.client, tool, &arguments) {
            Ok(value) => rpc_result(id, tool_success(value)),
            Err(error) => rpc_result(id, tool_failure(error.to_string())),
        }
    }
}

pub fn run_stdio(config: Config) -> Result<(), Error> {
    McpServer::new(config).run_stdio()
}

fn tool_success(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "isError": false,
        "content": [{"type": "text", "text": text}],
        "structuredContent": value
    })
}

fn tool_failure(message: String) -> Value {
    json!({
        "isError": true,
        "content": [{"type": "text", "text": message}]
    })
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
        // Nothing listens here; these tests never reach the network.
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
        for expected in [
            "memory_observe",
            "memory_recall",
            "memory_promote",
            "memory_project_share",
        ] {
            assert!(names.contains(&expected), "{expected}");
        }
        for removed in [
            "memory_account_bootstrap",
            "memory_account_register",
            "memory_project_invite",
            "memory_project_team_add",
            "memory_project_team_remove",
            "memory_sessions",
        ] {
            assert!(!names.contains(&removed), "{removed}");
        }
    }

    #[test]
    fn malformed_and_failed_tool_calls_are_protocol_safe() {
        let mut server = server();
        call(&mut server, 1, "initialize", json!({}));

        let missing = call(&mut server, 2, "tools/call", json!({"name": "memory_save"}));
        assert_eq!(missing["result"]["isError"], true);
        assert!(
            missing["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("content")
        );

        let unknown = call(&mut server, 3, "tools/call", json!({"name": "unknown"}));
        assert_eq!(unknown["error"]["code"], -32602);

        let unreachable = call(
            &mut server,
            4,
            "tools/call",
            json!({"name": "memory_diagnose", "arguments": {}}),
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
        call(&mut server, 1, "initialize", json!({}));

        let side_effect = json!({"jsonrpc": "2.0", "method": "tools/call",
            "params": {"name": "memory_diagnose", "arguments": {}}});
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
}
