#!/bin/sh
set -eu

store="$(mktemp "${TMPDIR:-/tmp}/cairn-layer3-store.XXXXXX")"
outbox="$(mktemp "${TMPDIR:-/tmp}/cairn-layer3-outbox.XXXXXX")"
rm -f "$store" "$outbox"
trap 'rm -f "$store" "$outbox"' EXIT INT TERM

python3 - "$store" "$outbox" <<'PY'
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

store, outbox = sys.argv[1:]
root = os.getcwd()
port = "3121"
base = f"http://127.0.0.1:{port}"
process = subprocess.Popen(
    [
        "cargo",
        "run",
        "-q",
        "-p",
        "cairn-server",
        "--",
        "--store",
        store,
        "--bind",
        f"127.0.0.1:{port}",
    ],
    cwd=root,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)


def call(path, payload=None, token=None):
    data = None if payload is None else json.dumps(payload).encode()
    headers = {"Content-Type": "application/json"} if data is not None else {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(base + path, data=data, headers=headers, method="POST" if data is not None else "GET")
    with urllib.request.urlopen(request, timeout=10) as response:
        return response.status, json.loads(response.read())


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
            time.sleep(0.1)
    else:
        raise RuntimeError("Layer 3 server did not become ready")

    _, account = call(
        "/cairn-memory/auth/bootstrap",
        {
            "email": "layer3-owner@example.test",
            "password": "correct horse battery staple",
        },
    )
    account_token = account["token"]
    person_id = account["account"]["person_id"]
    _, first = call(
        "/cairn-memory/devices/register",
        {"deviceId": "device-a", "personId": person_id, "agentId": "agent-a", "name": "MacBook"},
        account_token,
    )
    _, second = call(
        "/cairn-memory/devices/register",
        {"deviceId": "device-b", "personId": person_id, "name": "Laptop"},
        account_token,
    )
    token_a = first["token"]
    token_b = second["token"]
    event = {
        "eventId": "layer3-event-1",
        "sessionId": "layer3-session",
        "content": "Cross-device memory was captured. api_key=layer3-secret",
        "role": "agent",
        "source": "outbox",
        "context": {"repository": "cairn-demo", "branch": "main"},
    }

    subprocess.run(
        ["python3", "scripts/cairn_outbox.py", "enqueue", outbox],
        input=json.dumps(event),
        text=True,
        check=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )
    assert "layer3-secret" not in open(outbox, encoding="utf-8").read()
    replay = subprocess.run(
        ["python3", "scripts/cairn_outbox.py", "replay", outbox, "--url", base, "--token", token_a],
        text=True,
        check=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )
    assert json.loads(replay.stdout)["pending"] == 0
    status = subprocess.check_output(
        ["python3", "scripts/cairn_outbox.py", "status", outbox],
        text=True,
        cwd=root,
    )
    assert json.loads(status)["pending"] == 0

    offline_event = dict(event)
    offline_event["eventId"] = "layer3-event-offline"
    subprocess.run(
        ["python3", "scripts/cairn_outbox.py", "enqueue", outbox],
        input=json.dumps(offline_event),
        text=True,
        check=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )
    failed_replay = subprocess.run(
        [
            "python3",
            "scripts/cairn_outbox.py",
            "replay",
            outbox,
            "--url",
            "http://127.0.0.1:3999",
            "--token",
            token_a,
        ],
        text=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )
    assert failed_replay.returncode != 0
    status = subprocess.check_output(
        ["python3", "scripts/cairn_outbox.py", "status", outbox],
        text=True,
        cwd=root,
    )
    assert json.loads(status)["pending"] == 1
    subprocess.run(
        ["python3", "scripts/cairn_outbox.py", "replay", outbox, "--url", base, "--token", token_a],
        text=True,
        check=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )

    _, duplicate = call("/cairn-memory/sync/push", event, token_a)
    assert duplicate["duplicate"] is True

    _, pulled = call("/cairn-memory/sync/pull", {"cursor": 0}, token_b)
    assert len(pulled["events"]) == 2
    assert {event["event_id"] for event in pulled["events"]} == {
        "layer3-event-1",
        "layer3-event-offline",
    }
    assert "layer3-secret" not in json.dumps(pulled)
    cursor = pulled["cursor"]
    _, resumed = call("/cairn-memory/sync/pull", {"cursor": cursor}, token_b)
    assert resumed["events"] == []

    _, rotated = call("/cairn-memory/devices/rotate", {"deviceId": "device-a"}, token_a)
    token_a_rotated = rotated["token"]
    expect_unauthorized("/cairn-memory/sync/pull", {"cursor": 0}, token_a)
    revoked_event = dict(event)
    revoked_event["eventId"] = "layer3-event-revoked"
    subprocess.run(
        ["python3", "scripts/cairn_outbox.py", "enqueue", outbox],
        input=json.dumps(revoked_event),
        text=True,
        check=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )
    purged_replay = subprocess.run(
        [
            "python3",
            "scripts/cairn_outbox.py",
            "replay",
            outbox,
            "--url",
            base,
            "--token",
            token_a,
        ],
        text=True,
        cwd=root,
        stdout=subprocess.PIPE,
    )
    assert purged_replay.returncode != 0
    assert json.loads(purged_replay.stdout)["purged"] == 1
    assert json.loads(purged_replay.stdout)["pending"] == 0

    _, devices = call("/cairn-memory/devices", None, token_a_rotated)
    assert {device["device_id"] for device in devices} == {"device-a", "device-b"}
    assert all(device["last_seen_at"] for device in devices)

    _, revoked = call("/cairn-memory/devices/revoke", {"deviceId": "device-b"}, token_a_rotated)
    assert revoked["status"] == "revoked"
    expect_unauthorized("/cairn-memory/sync/pull", {"cursor": cursor}, token_b)
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)

print("Layer 3 device sync smoke check passed")
PY
