#!/bin/sh
set -eu

: "${CAIRN_DATABASE_URL:?set CAIRN_DATABASE_URL to a fresh PostgreSQL database}"

python3 - "$CAIRN_DATABASE_URL" <<'PY'
import json
import os
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

database_url = sys.argv[1]
root = os.getcwd()
port = "3122"
base = f"http://127.0.0.1:{port}"
log_path = os.path.join(
    "/private/var/folders/0q/vssmtzxj543g5v66d485tdrh0000gn/T",
    f"cairn-postgres-smoke-{os.getpid()}.log",
)
log = open(log_path, "w+", encoding="utf-8")
process = subprocess.Popen(
    [
        "cargo",
        "run",
        "-q",
        "-p",
        "cairn-server",
        "--",
        "--database-url",
        database_url,
        "--bind",
        f"127.0.0.1:{port}",
    ],
    cwd=root,
    stdout=log,
    stderr=subprocess.STDOUT,
    text=True,
    start_new_session=True,
)


def call(path, payload=None, token=None):
    data = None if payload is None else json.dumps(payload).encode()
    headers = {"Content-Type": "application/json"} if data is not None else {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        base + path,
        data=data,
        headers=headers,
        method="POST" if data is not None else "GET",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        body = response.read()
        return response.status, None if not body else json.loads(body)


def expect_unauthorized(path, payload, token):
    try:
        call(path, payload, token)
    except urllib.error.HTTPError as error:
        assert error.code == 401, error.code
        return
    raise AssertionError("request unexpectedly succeeded")


try:
    for _ in range(600):
        try:
            call("/cairn-memory/health")
            break
        except (urllib.error.URLError, TimeoutError):
            if process.poll() is not None:
                log.flush()
                log.seek(0)
                raise RuntimeError(
                    f"PostgreSQL server exited before readiness:\n{log.read()}"
                )
            time.sleep(0.1)
    else:
        log.flush()
        log.seek(0)
        raise RuntimeError(f"PostgreSQL server did not become ready:\n{log.read()}")

    _, account = call(
        "/cairn-memory/auth/bootstrap",
        {
            "email": "postgres-owner@example.test",
            "password": "correct horse battery staple",
        },
    )
    account_token = account["token"]
    person_id = account["account"]["person_id"]
    _, first = call(
        "/cairn-memory/devices/register",
        {"deviceId": "postgres-device-a", "personId": person_id, "name": "MacBook"},
        account_token,
    )
    _, second = call(
        "/cairn-memory/devices/register",
        {"deviceId": "postgres-device-b", "personId": person_id, "name": "Laptop"},
        account_token,
    )
    token_a = first["token"]
    token_b = second["token"]
    event = {
        "eventId": "postgres-event-1",
        "sessionId": "postgres-session",
        "content": "PostgreSQL sync captured api_key=secret",
        "role": "agent",
        "source": "smoke",
    }

    _, pushed = call("/cairn-memory/sync/push", event, token_a)
    assert pushed["duplicate"] is False
    assert "api_key=secret" not in json.dumps(pushed)
    _, duplicate = call("/cairn-memory/sync/push", event, token_a)
    assert duplicate["duplicate"] is True
    assert duplicate["cursor"] == pushed["cursor"]
    _, pulled = call("/cairn-memory/sync/pull", {"cursor": 0}, token_b)
    assert len(pulled["events"]) == 1
    assert "api_key=secret" not in json.dumps(pulled)
    _, sync_diagnostics = call("/cairn-memory/sync/diagnose", token=token_a)
    assert sync_diagnostics["server_cursor"] == pushed["cursor"]
    assert sync_diagnostics["lag"] == pushed["cursor"]

    _, remembered = call(
        "/cairn-memory/remember",
        {
            "content": "Redis cache is disabled because the server is authoritative",
            "concepts": ["retrieval", "authority"],
            "context": {"repository": "cairn", "branch": "feature/cache", "commit": "abc1234"},
        },
        account_token,
    )
    _, project = call(
        "/cairn-memory/projects",
        {"repository": "cairn", "name": "Cairn Memory", "defaultBranch": "main"},
        account_token,
    )
    _, branch = call(
        f"/cairn-memory/projects/{project['project_id']}/branches",
        {"name": "feature/cache", "baseBranch": "main", "baseCommit": "base123", "headCommit": "abc1234"},
        account_token,
    )
    _, worktree = call(
        f"/cairn-memory/projects/{project['project_id']}/worktrees",
        {"branchId": branch["branch_id"], "path": "/worktrees/cache", "commit": "abc1234"},
        account_token,
    )
    assert worktree["branch_id"] == branch["branch_id"]
    _, closable_branch = call(
        f"/cairn-memory/projects/{project['project_id']}/branches",
        {"name": "feature/obsolete", "baseBranch": "main"},
        account_token,
    )
    _, closed_branch = call(
        f"/cairn-memory/projects/{project['project_id']}/branches/{closable_branch['branch_id']}/close",
        {"reason": "Superseded before merge"},
        account_token,
    )
    assert closed_branch["status"] == "closed"
    _, promoted = call(
        "/cairn-memory/promote",
        {
            "projectId": project["project_id"],
            "memoryId": remembered["memory_id"],
            "reason": "Confirmed by the PostgreSQL retrieval smoke flow",
        },
        account_token,
    )
    assert promoted["kind"] == "decision"
    assert promoted["scope"] == "project"
    assert promoted["promoted_from"] == remembered["memory_id"]
    assert promoted["verified_commit"] == "abc1234"
    _, verified = call(
        "/cairn-memory/projects/verify",
        {
            "projectId": project["project_id"],
            "memoryId": promoted["memory_id"],
            "commit": "def5678",
        },
        account_token,
    )
    assert verified["verified_commit"] == "def5678"
    _, merged = call(
        f"/cairn-memory/projects/{project['project_id']}/branches/{branch['branch_id']}/merge",
        {
            "memoryId": remembered["memory_id"],
            "commit": "merge999",
            "reason": "Confirmed after merge into main",
        },
        account_token,
    )
    assert merged["branch"]["status"] == "merged"
    assert merged["memory"]["memory_id"] == promoted["memory_id"]
    assert merged["memory"]["verified_commit"] == "merge999"
    _, repeated_merge = call(
        f"/cairn-memory/projects/{project['project_id']}/branches/{branch['branch_id']}/merge",
        {
            "memoryId": remembered["memory_id"],
            "commit": "merge999",
            "reason": "Confirmed after merge into main",
        },
        account_token,
    )
    assert repeated_merge["memory"]["memory_id"] == promoted["memory_id"]
    _, branches = call(
        f"/cairn-memory/projects/{project['project_id']}/branches",
        token=account_token,
    )
    assert any(item["status"] == "merged" for item in branches)
    _, worktrees = call(
        f"/cairn-memory/projects/{project['project_id']}/worktrees",
        token=account_token,
    )
    assert worktrees[0]["status"] == "removed"
    _, project_memories = call(
        f"/cairn-memory/projects/{project['project_id']}/memories",
        token=account_token,
    )
    assert len(project_memories) == 1
    _, entities = call(
        f"/cairn-memory/projects/{project['project_id']}/entities",
        token=account_token,
    )
    assert any(
        entity["kind"] == "decision" and entity["key"] == promoted["memory_id"]
        for entity in entities
    )
    _, registered = call(
        f"/cairn-memory/projects/{project['project_id']}/entities",
        {
            "kind": "runbook",
            "key": "deploy",
            "content": "api_key=do-not-store",
            "sourceMemoryId": remembered["memory_id"],
            "commit": "abc1234",
        },
        account_token,
    )
    assert registered["content"] == "api_key=[REDACTED]"
    assert "do-not-store" not in registered["content"]
    _, repeated_entity = call(
        f"/cairn-memory/projects/{project['project_id']}/entities",
        {"kind": "runbook", "key": "deploy", "content": "a later edit"},
        account_token,
    )
    assert repeated_entity["entity_id"] == registered["entity_id"]
    assert repeated_entity["content"] == "api_key=[REDACTED]"
    _, history = call(
        f"/cairn-memory/projects/{project['project_id']}/freshness",
        token=account_token,
    )
    recorded_actions = {event["action"] for event in history}
    assert {"promotion", "verification", "merge"} <= recorded_actions
    _, stale = call(
        f"/cairn-memory/projects/{project['project_id']}/maintenance",
        {"currentCommit": "other-commit"},
        account_token,
    )
    assert promoted["memory_id"] in stale["stale_memory_ids"]
    assert promoted["memory_id"] in stale["active_memory_ids"]
    assert stale["superseded_memory_ids"] == []
    _, current = call(
        f"/cairn-memory/projects/{project['project_id']}/maintenance",
        {"currentCommit": "merge999"},
        account_token,
    )
    assert promoted["memory_id"] not in current["stale_memory_ids"]
    _, context = call(
        "/cairn-memory/context",
        {"query": "server authoritative", "limit": 1},
        account_token,
    )
    assert context["results"]
    _, why_changed = call(
        "/cairn-memory/why-changed",
        {"query": "server authoritative", "limit": 5},
        account_token,
    )
    assert any(result.get("decision_reason") for result in why_changed["results"])
    _, correction = call(
        "/cairn-memory/correct",
        {
            "targetId": remembered["memory_id"],
            "content": "Redis cache is disabled because the server owns canonical memory",
        },
        account_token,
    )
    assert correction["kind"] == "correction"
    assert correction["lifecycle"]["confidence"] == 100
    assert correction["supersedes"] == remembered["memory_id"]
    _, after_correction = call(
        "/cairn-memory/search",
        {"query": "authoritative", "limit": 5},
        account_token,
    )
    assert all(result["memory_id"] != remembered["memory_id"] for result in after_correction)
    _, canonical = call(
        "/cairn-memory/search",
        {"query": "canonical", "limit": 5},
        account_token,
    )
    assert any(result["memory_id"] == correction["memory_id"] for result in canonical)
    _, project_correction = call(
        "/cairn-memory/correct",
        {
            "targetId": promoted["memory_id"],
            "content": "Redis cache stays disabled because the server owns canonical memory",
            "lifecycle": {"confidence": 10},
        },
        account_token,
    )
    assert project_correction["lifecycle"]["confidence"] == 100
    assert project_correction["project_id"] == project["project_id"]
    assert project_correction["supersedes"] == promoted["memory_id"]
    _, listed_after_correction = call(
        f"/cairn-memory/projects/{project['project_id']}/memories",
        token=account_token,
    )
    assert any(
        memory["memory_id"] == project_correction["memory_id"]
        for memory in listed_after_correction
    )
    assert all(
        memory["memory_id"] != promoted["memory_id"] for memory in listed_after_correction
    )
    _, report = call(
        f"/cairn-memory/projects/{project['project_id']}/maintenance",
        {"currentCommit": "merge999"},
        account_token,
    )
    assert promoted["memory_id"] in report["superseded_memory_ids"]
    assert project_correction["memory_id"] in report["active_memory_ids"]
    assert promoted["memory_id"] not in report["active_memory_ids"]
    _, exported = call("/cairn-memory/export", {}, account_token)
    assert any(memory["memory_id"] == remembered["memory_id"] for memory in exported["memories"])
    assert remembered["memory_id"] not in exported["forgottenIds"]
    assert promoted["memory_id"] not in exported["forgottenIds"]
    _, imported = call("/cairn-memory/import", {"bundle": exported}, account_token)
    assert imported["imported_memories"] == 0
    _, lifecycle = call("/cairn-memory/diagnose", token=account_token)
    assert lifecycle["durable_memories"] >= 2
    _, audit = call("/cairn-memory/auth/audit", token=account_token)
    assert any(event["action"] == "account_bootstrap" for event in audit)

    _, rotated = call("/cairn-memory/devices/rotate", {"deviceId": "postgres-device-a"}, token_a)
    expect_unauthorized("/cairn-memory/sync/pull", {"cursor": 0}, token_a)
    token_a = rotated["token"]
    _, listed = call("/cairn-memory/devices", token=token_b)
    assert len(listed) == 2
    call("/cairn-memory/devices/revoke", {"deviceId": "postgres-device-a"}, token_b)
    expect_unauthorized("/cairn-memory/sync/pull", {"cursor": 0}, token_a)
finally:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
    log.close()
    try:
        os.unlink(log_path)
    except FileNotFoundError:
        pass

print("PostgreSQL Layer 3/4 smoke check passed")
PY
