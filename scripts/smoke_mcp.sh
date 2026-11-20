#!/bin/sh
set -eu

. "$(dirname "$0")/smoke_agent_lib.sh"

start_server 3134
create_agent mcp mcp@example.test bootstrap
use_agent mcp

CAIRN_BIN="$bin_dir/cairn" python3 - <<'PY'
import json
import os
import subprocess

# The server speaks to the device token in the CLI config; `cairn mcp` proxies to it.
process = subprocess.Popen(
    [os.environ["CAIRN_BIN"], "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)


def call(request_id, method, params):
    process.stdin.write(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            }
        )
        + "\n"
    )
    process.stdin.flush()
    line = process.stdout.readline()
    if not line:
        error = process.stderr.read()
        raise RuntimeError(f"MCP server closed stdout: {error}")
    return json.loads(line)


try:
    initialized = call(
        1,
        "initialize",
        {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cairn-smoke", "version": "0.1.0"},
        },
    )
    assert initialized["result"]["protocolVersion"] == "2025-06-18"
    assert initialized["result"]["serverInfo"]["name"] == "cairn-memory"
    assert initialized["result"]["serverInfo"]["title"] == "Cairn"
    instructions = initialized["result"]["instructions"]
    assert "cairn-memory" in instructions and "memory_save" in instructions

    process.stdin.write(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }
        )
        + "\n"
    )
    process.stdin.flush()

    tools = call(2, "tools/list", {})
    names = {tool["name"] for tool in tools["result"]["tools"]}
    assert {"memory_save", "memory_recall", "memory_observe"} <= names
    for removed in ("memory_promote", "memory_correct", "memory_import", "memory_review_dismiss",
                    "memory_project_verify", "memory_project_freshness"):
        assert removed not in names, removed
    schema = next(t["inputSchema"] for t in tools["result"]["tools"] if t["name"] == "memory_save")
    assert {"kind", "title", "summary", "reason"} <= set(schema["required"])
    record = {"scope":"global", "kind":"procedure", "title":"Scoped record", "summary":"Typed RPC record", "reason":"Round trip verification", "fields":{"steps":["one", "two"], "when":"testing"}}
    for index, field in enumerate(["kind", "title", "summary", "reason"]):
        missing = dict(record)
        del missing[field]
        response = call(20 + index, "tools/call", {"name":"memory_save", "arguments":missing})
        assert response["result"]["isError"] is True
        assert field in response["result"]["content"][0]["text"]
    unknown = call(30, "tools/call", {"name":"memory_save", "arguments":dict(record, colour="red")})
    assert unknown["result"]["isError"] is True and "colour" in unknown["result"]["content"][0]["text"]
    saved_result = call(24, "tools/call", {"name":"memory_save", "arguments":record})["result"]
    saved = saved_result["structuredContent"]
    assert set(saved) == {"id", "status", "deduplicated"}
    assert saved["status"] == "active" and saved["deduplicated"] is False
    assert saved_result["content"][0]["text"] == json.dumps(saved, separators=(",", ":"))
    loaded_result = call(25, "tools/call", {"name":"memory_get", "arguments":{"memoryId":saved["id"]}})["result"]
    loaded = loaded_result["structuredContent"]
    assert loaded["kind"] == "procedure" and loaded["scope"] == "global"
    for field in ("title", "summary", "reason", "fields"):
        assert loaded[field] == record[field], (field, loaded)
    assert "usage" not in loaded and "lifecycle" not in loaded
    assert "\n" not in loaded_result["content"][0]["text"]

    call(
        3,
        "tools/call",
        {
            "name": "memory_session_start",
            "arguments": {
                "sessionId": "mcp-smoke",
                "context": {
                    "repository": "cairn-demo",
                    "branch": "main",
                    "worktree": "wt-main",
                    "files": ["src/mcp.rs"],
                    "symbols": ["McpServer::run_stdio"],
                    "commands": ["cargo test -p cairn-server"],
                    "tests": ["mcp_smoke"],
                    "services": ["mcp"],
                },
            },
        },
    )
    captured = call(
        4,
        "tools/call",
        {
            "name": "memory_observe",
            "arguments": {
                "eventId": "mcp-smoke-event",
                "sessionId": "mcp-smoke",
                "content": "The MCP smoke test passed because the stdio protocol remained replay-safe.",
                "role": "agent",
                "source": "smoke",
            },
        },
    )
    assert captured["result"]["structuredContent"]["accepted"] is True

    ended = call(5, "tools/call", {"name": "memory_session_end", "arguments": {"sessionId": "mcp-smoke"}})
    assert ended["result"]["structuredContent"]["status"] == "queued"

    # A raw capture is never recalled; the saved record is.
    recalled = call(
        6,
        "tools/call",
        {"name": "memory_recall", "arguments": {"query": "scoped record typed RPC", "limit": 5}},
    )
    recalled_rows = recalled["result"]["structuredContent"]["results"]
    assert [m["id"] for m in recalled_rows] == [saved["id"]]
    assert set(recalled_rows[0]) == {"id", "kind", "scope", "title", "summary", "score"}
    assert "\n" not in recalled["result"]["content"][0]["text"]
    expanded = call(
        16,
        "tools/call",
        {"name": "memory_recall", "arguments": {"query": "scoped record typed RPC", "limit": 5, "expanded": True}},
    )
    expanded_row = expanded["result"]["structuredContent"]["results"][0]
    for hidden in ("usage", "evidence_hash", "identity_hash", "owner", "lifecycle", "content"):
        assert hidden not in expanded_row, hidden
    assert expanded_row["fields"]["when"] == "testing" and expanded_row["reason"] == "Round trip verification"
    nothing = call(31, "tools/call", {"name": "memory_recall", "arguments": {"query": "stdio protocol replay-safe", "limit": 5}})
    assert nothing["result"]["structuredContent"]["results"] == []

    newer = dict(record, title="Scoped record v2", summary="Typed RPC record, replaced", supersedes=saved["id"])
    newer_id = call(32, "tools/call", {"name":"memory_save", "arguments":newer})["result"]["structuredContent"]["id"]
    why_changed = call(
        7,
        "tools/call",
        {"name": "memory_why_changed", "arguments": {"query": "scoped record typed RPC", "limit": 5}},
    )
    why_rows = why_changed["result"]["structuredContent"]["results"]
    why_result = next(m for m in why_rows if m["id"] == newer_id)
    assert why_result["reason"] == "Round trip verification"
    assert any(l["relation"] == "supersedes" and l["source"] == newer_id for l in why_result["links"]), why_result
    assert all(m["id"] != saved["id"] for m in why_rows), "the replaced memory leaves recall"
    old = call(33, "tools/call", {"name": "memory_get", "arguments": {"memoryId": saved["id"]}})["result"]["structuredContent"]
    assert old["status"] == "superseded"

    listed = call(17, "tools/call", {"name": "memory_list", "arguments": {}})
    assert isinstance(listed["result"]["structuredContent"]["memories"], list)
    assert listed["result"]["structuredContent"]["memories"]
    graph = call(18, "tools/call", {"name": "memory_graph", "arguments": {}})
    graph_body = graph["result"]["structuredContent"]
    assert isinstance(graph_body["nodes"], list) and isinstance(graph_body["edges"], list)
    assert graph_body["edges"] and graph_body["edges"][0]["relation"] == "supersedes"
    assert graph_body["truncated"] is False and graph_body["node_cap"] >= 120
    graph_text = graph["result"]["content"][0]["text"]
    assert "nodes[" in graph_text and "\t" in graph_text and "truncated: false" in graph_text

    forgotten = call(8, "tools/call", {"name": "memory_forget", "arguments": {"id": newer_id}})
    assert forgotten["result"]["isError"] is False
    recalled = call(
        9,
        "tools/call",
        {"name": "memory_recall", "arguments": {"query": "scoped record typed RPC", "limit": 5}},
    )
    # Forgetting the newer memory brings the one it replaced back.
    assert [m["id"] for m in recalled["result"]["structuredContent"]["results"]] == [saved["id"]]
finally:
    process.stdin.close()
    process.wait(timeout=10)

print("MCP compatibility smoke check passed")
PY
