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
    assert len(recalled["result"]["structuredContent"]["results"]) == 1

    why_changed = call(
        7,
        "tools/call",
        {
            "name": "memory_why_changed",
            "arguments": {"query": "MCP smoke", "limit": 5, "context": {"repository": "cairn-demo"}},
        },
    )
    why_result = why_changed["result"]["structuredContent"]["results"][0]
    assert why_result["decision_reason"] == "the stdio protocol remained replay-safe"
    assert any(
        relationship["relation"] == "memory_touches_file"
        for relationship in why_result["relationships"]
    )

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
