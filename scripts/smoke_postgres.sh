#!/bin/sh
set -eu

: "${CAIRN_DATABASE_URL:?set CAIRN_DATABASE_URL to a fresh PostgreSQL database}"

python3 - "$CAIRN_DATABASE_URL" <<'PY'
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

database_url = sys.argv[1]
root = os.getcwd()
port = "3122"
base = f"http://127.0.0.1:{port}"
log_path = os.path.join(
    tempfile.gettempdir(),
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


def expect_error(path, payload, token, status):
    try:
        call(path, payload, token)
    except urllib.error.HTTPError as error:
        body = error.read().decode()
        assert error.code == status, (error.code, body)
        return body
    raise AssertionError(path + " unexpectedly succeeded")


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

    assert call("/cairn-memory/health")[1]["bootstrap_required"] is True
    _, account = call(
        "/cairn-memory/auth/bootstrap",
        {
            "email": "postgres-owner@example.test",
            "password": "correct horse battery staple",
        },
    )
    account_token = account["token"]
    person_id = account["account"]["person_id"]
    assert call("/cairn-memory/health")[1]["bootstrap_required"] is False
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

    _, member = call(
        "/cairn-memory/auth/register",
        {
            "email": "postgres-member@example.test",
            "password": "correct horse battery staple",
        },
        account_token,
    )
    member_token = member["token"]
    # Only the admin web session creates accounts; the first account is the admin.
    assert call("/cairn-memory/auth/me", token=account_token)[1]["is_admin"] is True
    assert call("/cairn-memory/auth/me", token=member_token)[1]["is_admin"] is False
    register_body = {
        "email": "postgres-open@example.test",
        "password": "correct horse battery staple",
    }
    expect_error("/cairn-memory/auth/register", register_body, None, 401)
    expect_error("/cairn-memory/auth/register", register_body, member_token, 403)
    _, private_memory = call(
        "/cairn-memory/remember",
        {"content": "private observation stays on the owner account"},
        account_token,
    )
    _, repository_memory = call(
        "/cairn-memory/remember",
        {
            "content": "repository cache decision is ready for the project",
            "context": {"repository": "cairn", "commit": "abc1234"},
        },
        account_token,
    )
    _, shared_promoted = call(
        "/cairn-memory/promote",
        {
            "projectId": project["project_id"],
            "memoryId": repository_memory["memory_id"],
            "reason": "Share the promoted repository decision",
        },
        account_token,
    )
    _, invited = call(
        f"/cairn-memory/projects/{project['project_id']}/members",
        {"email": "postgres-member@example.test"},
        account_token,
    )
    assert invited["person_id"] == member["account"]["person_id"]
    try:
        call(
            f"/cairn-memory/projects/{project['project_id']}/shares",
            {
                "memoryId": private_memory["memory_id"],
                "email": "postgres-member@example.test",
            },
            account_token,
        )
    except urllib.error.HTTPError as error:
        assert error.code == 400, error.code
    else:
        raise AssertionError("private observation share unexpectedly succeeded")
    call(
        f"/cairn-memory/projects/{project['project_id']}/shares",
        {
            "memoryId": shared_promoted["memory_id"],
            "email": "postgres-member@example.test",
        },
        account_token,
    )
    _, member_shared = call(
        f"/cairn-memory/projects/{project['project_id']}/shared",
        token=member_token,
    )
    assert [item["memory_id"] for item in member_shared] == [shared_promoted["memory_id"]]
    _, member_projects = call("/cairn-memory/projects", token=member_token)
    assert any(
        item["project_id"] == project["project_id"] for item in member_projects
    )
    _, member_search = call(
        "/cairn-memory/search",
        {"query": "private observation", "limit": 5},
        member_token,
    )
    assert member_search == []
    expect_unauthorized(
        f"/cairn-memory/projects/{project['project_id']}/memories",
        None,
        member_token,
    )
    expect_unauthorized(
        "/cairn-memory/correct",
        {
            "targetId": shared_promoted["memory_id"],
            "content": "A member must not rewrite the shared decision.",
        },
        member_token,
    )
    expect_unauthorized(
        "/cairn-memory/forget",
        {"id": shared_promoted["memory_id"]},
        member_token,
    )
    call(
        f"/cairn-memory/projects/{project['project_id']}/shares/{shared_promoted['memory_id']}/revoke",
        {"email": "postgres-member@example.test"},
        account_token,
    )
    _, after_revoke = call(
        f"/cairn-memory/projects/{project['project_id']}/shared",
        token=member_token,
    )
    assert after_revoke == []
    _, owner_memories = call(
        f"/cairn-memory/projects/{project['project_id']}/memories",
        token=account_token,
    )
    assert any(item["memory_id"] == shared_promoted["memory_id"] for item in owner_memories)
    _, share_audit = call("/cairn-memory/auth/audit", token=account_token)
    share_actions = {event["action"] for event in share_audit}
    assert "project_memory_shared" in share_actions
    assert "project_memory_share_revoked" in share_actions

    _, candidate_source = call(
        "/cairn-memory/remember",
        {
            "content": "manual policy keeps this project decision unshared",
            "context": {"repository": "cairn", "commit": "abc1234"},
        },
        account_token,
    )
    _, candidate_promoted = call(
        "/cairn-memory/promote",
        {
            "projectId": project["project_id"],
            "memoryId": candidate_source["memory_id"],
            "reason": "Manual policy must not share this decision",
        },
        account_token,
    )
    _, before_policy = call(
        f"/cairn-memory/projects/{project['project_id']}/shared",
        token=member_token,
    )
    assert all(
        item["memory_id"] != candidate_promoted["memory_id"] for item in before_policy
    )
    _, candidates = call(
        f"/cairn-memory/projects/{project['project_id']}/share-candidates?email=postgres-member%40example.test",
        token=account_token,
    )
    candidate = next(
        item for item in candidates if item["memory_id"] == candidate_promoted["memory_id"]
    )
    assert "not shared" in candidate["reason"]
    assert "manual" in candidate["not_auto_shared_reason"]
    call(
        f"/cairn-memory/projects/{project['project_id']}/policy",
        {"policy": "auto_nonsensitive_project_memory"},
        account_token,
    )
    # The owner's project list carries the policy; the member's does not.
    owner_list = call("/cairn-memory/projects", token=account_token)[1]
    listed = next(item for item in owner_list if item["project_id"] == project["project_id"])
    assert listed["role"] == "owner"
    assert listed["share_policy"] == "auto_nonsensitive_project_memory"
    member_list = call("/cairn-memory/projects", token=member_token)[1]
    listed = next(item for item in member_list if item["project_id"] == project["project_id"])
    assert listed["role"] == "member" and "share_policy" not in listed
    _, auto_source = call(
        "/cairn-memory/remember",
        {
            "content": "nonsensitive repository decision is ready for collaborators",
            "context": {"repository": "cairn", "commit": "def5678"},
        },
        account_token,
    )
    _, auto_promoted = call(
        "/cairn-memory/promote",
        {
            "projectId": project["project_id"],
            "memoryId": auto_source["memory_id"],
            "reason": "Automatic policy shares this nonsensitive decision",
        },
        account_token,
    )
    _, private_note = call(
        "/cairn-memory/remember",
        {"content": "private observation stays on the owner account"},
        account_token,
    )
    _, after_policy = call(
        f"/cairn-memory/projects/{project['project_id']}/shared",
        token=member_token,
    )
    shared_ids = {item["memory_id"] for item in after_policy}
    assert auto_promoted["memory_id"] in shared_ids
    assert private_note["memory_id"] not in shared_ids
    auto_read = next(
        item for item in after_policy if item["memory_id"] == auto_promoted["memory_id"]
    )
    assert auto_read["share_reason"] == "auto_nonsensitive_project_memory"
    assert auto_read["promoted_from"] is None
    assert auto_source["memory_id"] not in json.dumps(auto_read)
    _, private_search = call(
        "/cairn-memory/search",
        {"query": "private observation", "limit": 5},
        member_token,
    )
    assert private_search == []
    call(
        f"/cairn-memory/projects/{project['project_id']}/policy",
        {"policy": "manual"},
        account_token,
    )
    _, after_manual = call(
        f"/cairn-memory/projects/{project['project_id']}/shared",
        token=member_token,
    )
    assert any(item["memory_id"] == auto_promoted["memory_id"] for item in after_manual)

    _, stranger = call(
        "/cairn-memory/auth/register",
        {
            "email": "postgres-stranger@example.test",
            "password": "correct horse battery staple",
        },
        account_token,
    )
    stranger_token = stranger["token"]
    project_id = project["project_id"]
    team = f"/cairn-memory/projects/{project_id}/team"
    team_members = f"/cairn-memory/projects/{project_id}/team/members"
    shared_path = f"/cairn-memory/projects/{project_id}/shared"
    expect_error(team, None, stranger_token, 400)
    expect_error(shared_path, None, stranger_token, 400)
    expect_unauthorized(team, {"name": "Platform"}, member_token)
    expect_unauthorized(
        f"/cairn-memory/projects/{project_id}/shares",
        {
            "memoryId": auto_promoted["memory_id"],
            "email": "postgres-member@example.test",
        },
        member_token,
    )
    expect_unauthorized(
        f"/cairn-memory/projects/{project_id}/shares/{auto_promoted['memory_id']}/revoke",
        {"email": "postgres-member@example.test"},
        member_token,
    )
    expect_unauthorized(
        "/cairn-memory/correct",
        {
            "targetId": auto_promoted["memory_id"],
            "content": "A member must not rewrite the shared decision.",
        },
        member_token,
    )
    expect_unauthorized(
        "/cairn-memory/forget",
        {"id": auto_promoted["memory_id"]},
        member_token,
    )
    expect_unauthorized(
        "/cairn-memory/delete",
        {"id": auto_promoted["memory_id"]},
        member_token,
    )
    _, created_team = call(team, {"name": "Platform"}, account_token)
    assert created_team["name"] == "Platform"
    _, added_member = call(
        team_members,
        {"email": "postgres-member@example.test"},
        account_token,
    )
    assert added_member["email"] == "postgres-member@example.test"
    _, listed_team = call(team, token=member_token)
    assert listed_team["members"][0]["email"] == "postgres-member@example.test"
    expect_error(team, None, stranger_token, 400)
    _, forgotten_note = call(
        "/cairn-memory/remember",
        {"content": "owner forget tombstone stays stored"},
        account_token,
    )
    call("/cairn-memory/forget", {"id": forgotten_note["memory_id"]}, account_token)
    call(
        f"/cairn-memory/projects/{project_id}/team/members/remove",
        {"email": "postgres-member@example.test"},
        account_token,
    )
    _, former_shared = call(shared_path, token=member_token)
    assert former_shared == []
    _, before_delete = call("/cairn-memory/export", {}, account_token)
    assert any(
        memory["memory_id"] == auto_promoted["memory_id"]
        for memory in before_delete["memories"]
    )
    call("/cairn-memory/delete", {"id": auto_promoted["memory_id"]}, account_token)
    _, after_delete_memories = call(
        f"/cairn-memory/projects/{project_id}/memories",
        token=account_token,
    )
    assert all(
        memory["memory_id"] != auto_promoted["memory_id"]
        for memory in after_delete_memories
    )
    _, deleted_search = call(
        "/cairn-memory/search",
        {"query": "nonsensitive repository decision", "limit": 5},
        account_token,
    )
    assert all(
        item["memory_id"] != auto_promoted["memory_id"] for item in deleted_search
    )
    _, after_delete = call("/cairn-memory/export", {}, account_token)
    assert any(
        memory["memory_id"] == auto_promoted["memory_id"]
        for memory in after_delete["memories"]
    )
    assert auto_promoted["memory_id"] in after_delete["forgottenIds"]
    _, team_audit = call("/cairn-memory/auth/audit", token=account_token)
    team_actions = {event["action"] for event in team_audit}
    assert "project_team_created" in team_actions
    assert "project_team_member_added" in team_actions
    assert "project_team_member_removed" in team_actions
    assert "memory_delete_retained" in team_actions
    assert "memory_forget" in team_actions
    audit_text = json.dumps(team_audit)
    assert "nonsensitive repository decision is ready for collaborators" not in audit_text
    assert "correct horse battery staple" not in audit_text

    # A device token is an agent credential for this account; device management and
    # account auth stay web-session only.
    _, device_projects = call("/cairn-memory/projects", token=token_b)
    assert any(item["project_id"] == project["project_id"] for item in device_projects)
    for method_path, payload in (
        ("/cairn-memory/devices", None),
        ("/cairn-memory/devices/register", {"name": "denied"}),
        ("/cairn-memory/devices/rotate", {"deviceId": "postgres-device-a"}),
        ("/cairn-memory/devices/revoke", {"deviceId": "postgres-device-a"}),
        ("/cairn-memory/auth/audit", None),
        ("/cairn-memory/auth/logout", {}),
    ):
        expect_error(method_path, payload, token_b, 403)

    _, rotated = call("/cairn-memory/devices/rotate", {"deviceId": "postgres-device-a"}, account_token)
    expect_unauthorized("/cairn-memory/sync/pull", {"cursor": 0}, token_a)
    token_a = rotated["token"]
    _, listed = call("/cairn-memory/devices", token=account_token)
    assert len(listed) == 2
    assert token_a not in json.dumps(listed)
    call("/cairn-memory/devices/revoke", {"deviceId": "postgres-device-a"}, account_token)
    expect_unauthorized("/cairn-memory/sync/pull", {"cursor": 0}, token_a)
    expect_unauthorized("/cairn-memory/projects", None, token_a)
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
