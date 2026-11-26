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
    assert len(instructions.encode()) <= 700, len(instructions.encode())
    assert "memory_search" in instructions and "memory_save" in instructions and "memory_get" in instructions

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
    names = [tool["name"] for tool in tools["result"]["tools"]]
    assert names == ["memory_search", "memory_get", "memory_save", "memory_feedback", "memory_forget",
                     "skill_search", "skill_get", "skill_save", "skill_feedback"], names
    memory_tools = {"tools": tools["result"]["tools"][:5]}
    assert len(json.dumps(memory_tools, separators=(",", ":"))) <= 3500
    schema = next(t["inputSchema"] for t in tools["result"]["tools"] if t["name"] == "memory_save")
    assert "required" not in schema
    assert schema["properties"]["items"]["items"]["required"] == ["kind", "title", "summary"]


    def result_json(response):
        result = response["result"]
        assert set(result) == {"content"}, result
        return json.loads(result["content"][0]["text"])


    record = {"scope":"global", "kind":"procedure", "title":"Save a scoped record", "summary":"Typed RPC record for the smoke test", "reason":"Round trip verification", "fields":{"steps":["one", "two"], "when":"testing"}}
    for index, field in enumerate(["kind", "title", "summary", "reason"]):
        missing = dict(record)
        del missing[field]
        response = call(20 + index, "tools/call", {"name":"memory_save", "arguments":missing})
        assert response["result"]["isError"] is True
        assert field in response["result"]["content"][0]["text"]
    unknown = call(30, "tools/call", {"name":"memory_save", "arguments":dict(record, colour="red")})
    assert unknown["result"]["isError"] is True and "colour" in unknown["result"]["content"][0]["text"]
    saved = result_json(call(24, "tools/call", {"name":"memory_save", "arguments":record}))
    assert set(saved) == {"id", "status", "deduplicated"}
    assert saved["status"] == "active" and saved["deduplicated"] is False

    loaded_result = call(25, "tools/call", {"name":"memory_get", "arguments":{"ids":[saved["id"], "mem_unknown"]}})
    loaded_all = result_json(loaded_result)
    loaded = loaded_all["memories"][0]
    assert loaded_all["missing"] == ["mem_unknown"]
    assert loaded["kind"] == "procedure" and loaded["scope"] == "global"
    for field in ("title", "summary", "reason", "fields"):
        assert loaded[field] == record[field], (field, loaded)
    assert "usage" not in loaded and "lifecycle" not in loaded
    assert "\n" not in loaded_result["result"]["content"][0]["text"]

    searched = call(6, "tools/call", {"name": "memory_search", "arguments": {"query": "scoped record typed RPC", "limit": 5}})
    rows = result_json(searched)["results"]
    assert [m["id"] for m in rows] == [saved["id"]]
    assert set(rows[0]) == {"id", "kind", "title", "summary", "score", "trust"}
    assert loaded["trust"] == "normal" and rows[0]["trust"] == "normal · used 1×"
    assert "\n" not in searched["result"]["content"][0]["text"]
    by_kind = result_json(call(16, "tools/call", {"name": "memory_search", "arguments": {"query": "scoped record typed RPC", "kind": "rule"}}))
    assert by_kind["results"] == []

    newer = dict(record, title="Scoped record v2", summary="This typed RPC record replaces the previous version.", supersedes=saved["id"])
    newer_id = result_json(call(32, "tools/call", {"name":"memory_save", "arguments":newer}))["id"]
    rows = result_json(call(7, "tools/call", {"name": "memory_search", "arguments": {"query": "scoped record typed RPC", "limit": 5}}))["results"]
    ids = [m["id"] for m in rows]
    assert ids[0] == newer_id and saved["id"] in ids, ids
    old_card = next(m for m in rows if m["id"] == saved["id"])
    assert old_card["status"] == "superseded"
    assert old_card["label"] == f"replaced by {newer_id}", old_card
    old = result_json(call(33, "tools/call", {"name": "memory_get", "arguments": {"ids": [saved["id"]]}}))["memories"][0]
    assert old["status"] == "superseded"
    assert any(l["relation"] == "supersedes" and l["source"] == newer_id for l in old["links"]), old

    voted = result_json(call(17, "tools/call", {"name": "memory_feedback", "arguments": {"id": newer_id, "vote": "helpful"}}))
    assert voted == {"id": newer_id, "status": "active"}
    forgotten = result_json(call(8, "tools/call", {"name": "memory_forget", "arguments": {"id": newer_id}}))
    assert forgotten == {"id": newer_id, "status": "forgotten"}
    gone = result_json(call(9, "tools/call", {"name": "memory_get", "arguments": {"ids": [newer_id]}}))
    assert gone["missing"] == [newer_id]
    skill = {"name": "e2e-skill-compile", "title": "E2E compile exercises", "description": "Practice crates compilation",
             "body": "Run the build before review. token=abc123", "change_note": "E2E initial skill"}
    first_skill = result_json(call(40, "tools/call", {"name": "skill_save", "arguments": skill}))
    assert first_skill["version"] == 1
    matches = result_json(call(41, "tools/call", {"name": "skill_search", "arguments": {"query": "crates compilation"}}))
    assert [item["id"] for item in matches["results"]] == [first_skill["id"]]
    read = result_json(call(42, "tools/call", {"name": "skill_get", "arguments": {"name_or_id": skill["name"], "version": 1}}))
    assert "Run the build" in read["body"] and "abc123" not in read["body"]
    vote = dict(name_or_id=first_skill["id"], version=1, vote="helpful")
    rated = result_json(call(43, "tools/call", {"name": "skill_feedback", "arguments": vote}))
    assert rated["votes"] == {"helpful": 1, "wrong": 0}

    def chat_skill(name, args):
        payload = {"session_id": "hchat_" + "a" * 32,
                   "chat_context": {"platform": "telegram", "kind": "group", "chat_id_hash": "a" * 64},
                   "skill_tool": name, "arguments": dict(args, context={"chat_id_hash": "b" * 64}, sender_id="raw-id")}
        reply = subprocess.run([os.environ["CAIRN_BIN"], "hook", "ChatSkill", "--agent", "hermes"],
                               input=json.dumps(payload), text=True, capture_output=True, check=True)
        return json.loads(reply.stdout)

    chat_saved = chat_skill("skill_save", dict(skill, body="Chat-saved body. token=zzz999", change_note="E2E chat edit"))
    assert chat_saved["version"] == 2, chat_saved
    chat_read = chat_skill("skill_get", {"name_or_id": first_skill["id"]})
    assert "Chat-saved body" in chat_read["body"] and "zzz999" not in chat_read["body"]
    assert "chat_id_hash" not in chat_read and "sender_id" not in chat_read
    assert chat_skill("skill_search", {"query": "crates compilation"})["results"][0]["id"] == first_skill["id"]
    assert chat_skill("skill_feedback", dict(vote, vote="wrong"))["votes"] == {"helpful": 0, "wrong": 1}
finally:
    process.stdin.close()
    process.wait(timeout=10)

print("MCP compatibility smoke check passed")
PY
