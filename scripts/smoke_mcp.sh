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
    assert "memory_promote" not in names
    schema = next(t["inputSchema"] for t in tools["result"]["tools"] if t["name"] == "memory_save")
    assert {"scope", "title", "summary", "reason"} <= set(schema["required"])
    record = {"scope":"global", "kind":"procedure", "title":"Scoped record", "summary":"Typed RPC record", "reason":"Round trip verification", "steps":["one", "two"], "when":"testing", "pinned":True}
    for index, field in enumerate(["scope", "title", "summary", "reason"]):
        missing = dict(record)
        del missing[field]
        response = call(20 + index, "tools/call", {"name":"memory_save", "arguments":missing})
        assert response["result"]["isError"] is True
        assert field in response["result"]["content"][0]["text"]
    saved_result = call(24, "tools/call", {"name":"memory_save", "arguments":record})["result"]
    saved = saved_result["structuredContent"]
    assert set(saved) == {"memory_id", "kind", "scope", "status"}
    assert saved["kind"] == "procedure" and saved["scope"] == "global" and saved["status"] == "active"
    assert saved_result["content"][0]["text"] == json.dumps(saved, separators=(",", ":"))
    loaded_result = call(25, "tools/call", {"name":"memory_get", "arguments":{"memoryId":saved["memory_id"]}})["result"]
    loaded = loaded_result["structuredContent"]
    for field, value in record.items():
        assert loaded[field] == value, (field, loaded)
    assert loaded["content"] and "lifecycle" in loaded
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

    ended = call(
        5,
        "tools/call",
        {
            "name": "memory_session_end",
            "arguments": {"sessionId": "mcp-smoke"},
        },
    )
    assert ended["result"]["structuredContent"]["source_event_ids"] == [
        "mcp-smoke-event"
    ]

    recalled = call(
        6,
        "tools/call",
        {
            "name": "memory_recall",
            "arguments": {"query": "MCP smoke", "limit": 5, "context": {"repository": "cairn-demo"}},
        },
    )
    recalled_rows = recalled["result"]["structuredContent"]["results"]
    assert {m["memory_id"] for m in recalled_rows} == {
        ended["result"]["structuredContent"]["memory_id"],
    }
    session_card = next(m for m in recalled_rows if m["memory_id"] == ended["result"]["structuredContent"]["memory_id"])
    assert set(session_card) == {"memory_id", "kind", "scope", "title", "summary", "score"}
    assert session_card["kind"] == "session_summary"
    assert "\n" not in recalled["result"]["content"][0]["text"]
    expanded = call(
        16,
        "tools/call",
        {
            "name": "memory_recall",
            "arguments": {"query": "MCP smoke", "limit": 5, "expanded": True, "context": {"repository": "cairn-demo"}},
        },
    )
    expanded_row = next(m for m in expanded["result"]["structuredContent"]["results"]
                        if m["memory_id"] == ended["result"]["structuredContent"]["memory_id"])
    assert "lifecycle" not in expanded_row and "content" not in expanded_row and "source_event_ids" not in expanded_row
    assert "stdio protocol" in expanded_row["details"]
    assert expanded["result"]["structuredContent"]["omitted_count"] == recalled["result"]["structuredContent"]["omitted_count"]

    why_changed = call(
        7,
        "tools/call",
        {
            "name": "memory_why_changed",
            "arguments": {"query": "MCP smoke", "limit": 5, "context": {"repository": "cairn-demo"}},
        },
    )
    why_result = next(m for m in why_changed["result"]["structuredContent"]["results"]
                      if m["memory_id"] == ended["result"]["structuredContent"]["memory_id"])
    assert why_result["decision_reason"] == "the stdio protocol remained replay-safe"
    assert any(
        relationship["relation"] == "memory_touches_file"
        for relationship in why_result["relationships"]
    )

    listed = call(17, "tools/call", {"name": "memory_list", "arguments": {}})
    assert isinstance(listed["result"]["structuredContent"]["memories"], list)
    assert listed["result"]["structuredContent"]["memories"]
    graph = call(18, "tools/call", {"name": "memory_graph", "arguments": {}})
    graph_body = graph["result"]["structuredContent"]
    assert isinstance(graph_body["nodes"], list) and isinstance(graph_body["edges"], list)
    assert graph_body["truncated"] is False and graph_body["node_cap"] >= 120
    graph_text = graph["result"]["content"][0]["text"]
    assert "nodes[" in graph_text and "\t" in graph_text and "truncated: false" in graph_text
    reviews = call(19, "tools/call", {"name": "memory_similarity_reviews", "arguments": {}})
    assert isinstance(reviews["result"]["structuredContent"]["reviews"], list)
    review_text = reviews["result"]["content"][0]["text"]
    assert review_text == "reviews: []" or review_text.startswith("reviews[")

    forgotten = call(
        8,
        "tools/call",
        {
            "name": "memory_forget",
            "arguments": {"eventId": "mcp-smoke-event"},
        },
    )
    assert forgotten["result"]["isError"] is False

    recalled = call(
        9,
        "tools/call",
        {
            "name": "memory_recall",
            "arguments": {"query": "MCP smoke", "limit": 5, "context": {"repository": "cairn-demo"}},
        },
    )
    assert recalled["result"]["structuredContent"]["results"] == []
finally:
    process.stdin.close()
    process.wait(timeout=10)

print("MCP compatibility smoke check passed")
PY
