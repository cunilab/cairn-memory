#!/usr/bin/env python3
"""Device-only offline hook enqueue. Loopback stubs and temp files only.

No live API, database, or repository store. Tokens stay in the environment,
not on a command line.
"""

from __future__ import annotations

import fcntl
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / "scripts" / "cairn_hook.py"
OUTBOX_PATH = ROOT / "scripts" / "cairn_outbox.py"
DEVICE = "device-token-hook7b"
OTHER = "other-device-token-hook7b"
ACCOUNT = "account-token-hook7b"
SECRET = "sk-hook7b-secret"
PEM = "hook7bpembody"
LISTS = ("files", "symbols", "commands", "tests", "services", "handoffs", "runbooks")
QUEUED = "Cairn hook queued for replay"
NOT_RETAINED = "the capture was not retained"


def load_outbox():
    spec = importlib.util.spec_from_file_location("cairn_outbox_offline", OUTBOX_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load cairn_outbox.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


OUTBOX = load_outbox()


def fingerprint(session_id, content, source):
    encoded = json.dumps(
        {"sessionId": session_id, "content": OUTBOX.redact_content(content), "source": source},
        sort_keys=True,
    ).encode()
    return "hook-" + hashlib.sha256(encoded).hexdigest()[:24]


def rich_payload(event_id=None):
    content = "decision api_key=%s and %s" % (SECRET, SECRET)
    context = {"repository": "cairn", "branch": "sk-looking-name"}
    for name in LISTS:
        context[name] = ["kept-" + name, "token=%s" % SECRET]
    context["runbooks"] = [
        "kept-runbooks",
        "-----BEGIN PRIVATE KEY-----\n%s\n-----END PRIVATE KEY-----" % PEM,
    ]
    payload = {
        "sessionId": "offline-session",
        "content": content,
        "role": "tool",
        "source": "PostToolUse",
        "context": context,
    }
    if event_id is not None:
        payload["eventId"] = event_id
    return payload


def run_hook(url, payload, spool, timeout=20, **env):
    child = os.environ.copy()
    for name in ("CAIRN_ACCOUNT_TOKEN", "CAIRN_DEVICE_TOKEN", "CAIRN_OUTBOX"):
        child.pop(name, None)
    child["CAIRN_URL"] = url
    if spool is not None:
        child["CAIRN_OUTBOX"] = str(spool)
    for key, value in env.items():
        if value is None:
            child.pop(key, None)
        else:
            child[key] = value
    if not isinstance(payload, str):
        payload = json.dumps(payload)
    started = time.monotonic()
    result = subprocess.run(
        [sys.executable, str(HOOK)],
        input=payload,
        text=True,
        capture_output=True,
        env=child,
        cwd=ROOT,
        timeout=timeout,
        check=False,
    )
    result.elapsed = time.monotonic() - started
    return result


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        self.server.posts.append(
            {
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
                "body": body,
            }
        )
        if self.server.chmod_dir is not None:
            os.chmod(self.server.chmod_dir, 0o555)
        mode = self.server.mode
        if mode == "stall":
            self._remember(body)
            self.server.stall.wait(30)
            self._send(200, '{"accepted":true}')
            return
        if mode == "partial":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", "80")
            self.end_headers()
            self.wfile.write(b"{}")
            self.close_connection = True
            return
        if mode == "garbage":
            self.wfile.write(b"garbage\r\n\r\n")
            self.close_connection = True
            return
        if mode == "redirect":
            data = b"{}"
            self.send_response(302)
            self.send_header("Location", self.server.location)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        if self.server.hold_lock is not None and self.server.lock_fd is None:
            # Take the queue lock after the hook's preflight, before it can replay.
            self.server.lock_fd = os.open(self.server.hold_lock, os.O_CREAT | os.O_RDWR, 0o600)
            fcntl.flock(self.server.lock_fd, fcntl.LOCK_EX)
        if mode == "accept_then_cut" and len(self.server.posts) > 1:
            self.close_connection = True
            return
        if mode == "accept_then_401" and len(self.server.posts) > 1:
            self._send(401, "{}")
            return
        if mode in ("accept", "accept_then_cut", "accept_then_401"):
            self._remember(body)
            self._send(200, '{"accepted":true,"duplicate":true}')
            return
        self._send(self.server.status, "{}")

    def _remember(self, body):
        try:
            event_id = json.loads(body)["eventId"]
        except (ValueError, KeyError, TypeError):
            event_id = "unparsed"
        if event_id in self.server.rows:
            self.server.kept += 1
            return
        self.server.rows[event_id] = body

    def _send(self, status, payload):
        data = payload.encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        return


class Stub:
    def __init__(self, mode="status", status=500):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.posts = []
        self.server.rows = {}
        self.server.kept = 0
        self.server.mode = mode
        self.server.status = status
        self.server.location = "http://127.0.0.1:9/stolen"
        self.server.chmod_dir = None
        self.server.stall = threading.Event()
        self.server.hold_lock = None
        self.server.lock_fd = None
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = "http://127.0.0.1:%d" % self.server.server_address[1]

    def close(self):
        if self.server.lock_fd is not None:
            os.close(self.server.lock_fd)
        self.server.stall.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class OfflineHookTests(unittest.TestCase):
    def assert_hidden(self, text, *needles):
        for needle in needles:
            if needle and needle in text:
                self.fail("output exposed a credential or payload value")

    def assert_no_argv_token(self, result):
        self.assertNotIn(DEVICE, result.args)
        self.assertNotIn(ACCOUNT, result.args)

    def assert_queue_clean(self, path, result):
        raw = path.read_bytes()
        self.assertNotIn(DEVICE.encode(), raw)
        self.assertNotIn(SECRET.encode(), raw)
        self.assertNotIn(PEM.encode(), raw)
        self.assertNotIn(OUTBOX.device_fingerprint(DEVICE).encode(), result.stdout.encode() + result.stderr.encode())
        self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET, PEM, path.as_posix())
        self.assert_no_argv_token(result)
        temps = [item.name for item in path.parent.iterdir() if item.name.endswith(".tmp")]
        self.assertEqual(temps, [])
        return raw

    def test_retryable_failures_queue_one_redacted_event(self):
        cases = (
            ("status", 500, None),
            ("status", 502, None),
            ("status", 408, None),
            ("status", 429, None),
            ("partial", 200, None),
            ("garbage", 200, None),
        )
        for mode, status, _note in cases:
            with self.subTest(mode=mode, status=status):
                stub = Stub(mode=mode, status=status)
                try:
                    with tempfile.TemporaryDirectory() as directory:
                        path = Path(directory) / "queue.json"
                        result = run_hook(
                            stub.url + "/",
                            rich_payload(),
                            path,
                            CAIRN_DEVICE_TOKEN=DEVICE,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertLess(result.elapsed, 5, result.stderr)
                        self.assertEqual(result.stderr.strip(), QUEUED)
                        self.assertEqual(result.stdout, "")
                        self.assertEqual(len(stub.server.posts), 1)
                        posted = stub.server.posts[0]
                        self.assertEqual(posted["path"], "/cairn-memory/sync/push")
                        self.assertEqual(posted["authorization"], "Bearer " + DEVICE)
                        body = json.loads(posted["body"])
                        content = rich_payload()["content"]
                        self.assertEqual(
                            body["eventId"],
                            fingerprint("offline-session", content, "PostToolUse"),
                        )
                        self.assertNotIn(SECRET, posted["body"].decode())
                        raw = self.assert_queue_clean(path, result)
                        state = json.loads(raw)
                        self.assertEqual(state["version"], 2)
                        self.assertEqual(state["route"], "sync/push")
                        self.assertEqual(state["origin"], stub.url)
                        self.assertEqual(len(state["events"]), 1)
                        stored = state["events"][0]
                        self.assertEqual(stored["attempts"], 0)
                        self.assertIsNone(stored["lastError"])
                        self.assertEqual(stored["payload"]["eventId"], body["eventId"])
                        self.assertIn("[REDACTED]", stored["payload"]["content"])
                        self.assertEqual(stored["payload"]["context"]["branch"], "sk-looking-name")
                        self.assertEqual(stored["payload"]["context"]["repository"], "cairn")
                        for name in LISTS:
                            self.assertEqual(stored["payload"]["context"][name][0], "kept-" + name)
                            self.assertIn("[REDACTED]", stored["payload"]["context"][name][1])
                        self.assertNotIn(OUTBOX.device_fingerprint(DEVICE), result.stdout + result.stderr)
                finally:
                    stub.close()

    def test_connection_refused_queues_once_and_keeps_the_same_id(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "queue.json"
            payload = rich_payload()
            first = run_hook("http://127.0.0.1:9", payload, path, CAIRN_DEVICE_TOKEN=DEVICE)
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertLess(first.elapsed, 5)
            self.assertEqual(first.stderr.strip(), QUEUED)
            raw = self.assert_queue_clean(path, first)
            state = json.loads(raw)
            event_id = state["events"][0]["payload"]["eventId"]
            self.assertEqual(
                event_id,
                fingerprint("offline-session", payload["content"], "PostToolUse"),
            )
            second = run_hook("http://127.0.0.1:9", payload, path, CAIRN_DEVICE_TOKEN=DEVICE)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertEqual(second.stderr.strip(), QUEUED)
            again = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(len(again["events"]), 1)
            self.assertEqual(again["events"][0]["payload"]["eventId"], event_id)
            self.assert_hidden(second.stdout + second.stderr, DEVICE, SECRET, PEM)

    def test_queued_event_keeps_the_context_derived_from_cwd(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "checkout"
            repo.mkdir()
            git = ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@example.test"]
            subprocess.run(git + ["init", "-q", "-b", "main"], check=True)
            subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", "x"], check=True)
            subprocess.run(git + ["remote", "add", "origin", "https://example.test/acme/widget-svc.git"], check=True)
            path = Path(directory) / "queue.json"
            payload = {"session_id": "cwd-session", "hook_event_name": "UserPromptSubmit", "prompt": "p", "cwd": str(repo)}
            result = run_hook("http://127.0.0.1:9", payload, path, CAIRN_DEVICE_TOKEN=DEVICE)
            self.assertEqual(result.returncode, 0, result.stderr)
            queued = json.loads(path.read_text(encoding="utf-8"))["events"][0]["payload"]
            self.assertEqual(queued["context"], {"repository": "example.test/acme/widget-svc", "branch": "main"})

    def test_timeout_after_store_replays_as_one_duplicate(self):
        stub = Stub(mode="stall")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                payload = rich_payload("timeout-event")
                result = run_hook(
                    stub.url,
                    payload,
                    path,
                    timeout=25,
                    CAIRN_DEVICE_TOKEN=DEVICE,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertGreater(result.elapsed, 8)
                self.assertLess(result.elapsed, 16)
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(result.stderr.strip(), QUEUED)
                self.assert_queue_clean(path, result)
                self.assertEqual(len(stub.server.rows), 1)
                stored = stub.server.rows["timeout-event"]
                stub.server.mode = "accept"
                stub.server.stall.set()
                previous_device = os.environ.get("CAIRN_DEVICE_TOKEN")
                previous_account = os.environ.get("CAIRN_ACCOUNT_TOKEN")
                os.environ["CAIRN_DEVICE_TOKEN"] = "wrong-env-device-token"
                os.environ["CAIRN_ACCOUNT_TOKEN"] = ACCOUNT
                try:
                    code, report = self.replay(path, stub.url, DEVICE)
                finally:
                    self._restore("CAIRN_DEVICE_TOKEN", previous_device)
                    self._restore("CAIRN_ACCOUNT_TOKEN", previous_account)
                self.assertEqual(code, 0, report)
                self.assertEqual(json.loads(report)["accepted"], 1)
                self.assertEqual(json.loads(report)["purged"], 0)
                self.assertEqual(len(stub.server.posts), 2)
                self.assertEqual(stub.server.rows["timeout-event"], stored)
                self.assertEqual(len(stub.server.rows), 1)
                self.assertGreaterEqual(stub.server.kept, 1)
                self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["events"], [])
                self.assertNotIn(DEVICE, report)
                code, report = self.replay(path, stub.url, DEVICE)
                self.assertEqual(code, 0, report)
                self.assertEqual(len(stub.server.posts), 2)
                self.assertEqual(json.loads(report)["accepted"], 0)
        finally:
            stub.close()

    def replay(self, path, url, token):
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with redirect_stdout(buffer):
            code = OUTBOX.replay(path, url, token)
        return code, buffer.getvalue()

    def _restore(self, name, value):
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value

    def test_permanent_responses_and_other_modes_do_not_queue(self):
        codes = (400, 401, 403, 404, 409, 422, 301, 303, 307, 308)
        for code in codes:
            with self.subTest(code=code):
                self.assert_status_does_not_queue(code)
        stub = Stub(mode="redirect")
        target = Stub(mode="status", status=200)
        try:
            stub.server.location = target.url + "/cairn-memory/sync/push"
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                before = self.write_bound(path, stub.url, "kept-redirect")
                result = run_hook(
                    stub.url,
                    rich_payload("redirect-event"),
                    path,
                    CAIRN_DEVICE_TOKEN=DEVICE,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(QUEUED, result.stderr)
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(target.server.posts, [])
                self.assertEqual(path.read_bytes(), before)
                self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET, PEM, target.url)
                self.assert_no_argv_token(result)
        finally:
            stub.close()
            target.close()

    def assert_status_does_not_queue(self, code):
        stub = Stub(mode="status", status=code)
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                before = self.write_bound(path, stub.url, "kept-%s" % code)
                result = run_hook(
                    stub.url,
                    rich_payload("permanent-%s" % code),
                    path,
                    CAIRN_DEVICE_TOKEN=DEVICE,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(QUEUED, result.stderr)
                self.assertNotIn("not retained", result.stderr)
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(path.read_bytes(), before)
                self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET, PEM)
        finally:
            stub.close()

    def test_success_does_not_queue_and_unset_still_drops(self):
        stub = Stub(mode="status", status=200)
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                result = run_hook(stub.url, rich_payload("ok-event"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, "{}\n")
                self.assertFalse(path.exists())
                self.assertEqual(len(stub.server.posts), 1)
                self.assertNotIn(QUEUED, result.stderr)
                self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET, PEM)
        finally:
            stub.close()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "queue.json"
            dropped = run_hook(
                "http://127.0.0.1:9",
                rich_payload("dropped"),
                None,
                CAIRN_DEVICE_TOKEN=DEVICE,
            )
            self.assertNotEqual(dropped.returncode, 0)
            self.assertIn("delivery failed", dropped.stderr)
            self.assertNotIn(QUEUED, dropped.stderr)
            self.assertFalse(path.exists())
            self.assert_hidden(dropped.stdout + dropped.stderr, DEVICE, SECRET)

    def test_missing_device_token_neither_posts_nor_queues(self):
        stub = Stub(mode="status", status=500)
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                anonymous = run_hook(stub.url, rich_payload("anon"), path)
                account = run_hook(
                    stub.url,
                    rich_payload("account"),
                    path,
                    CAIRN_ACCOUNT_TOKEN=ACCOUNT,
                )
                for result in (anonymous, account):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("CAIRN_DEVICE_TOKEN is required", result.stderr)
                    self.assertNotIn(QUEUED, result.stderr)
                    self.assert_hidden(result.stdout + result.stderr, DEVICE, ACCOUNT, SECRET, PEM)
                    self.assert_no_argv_token(result)
                self.assertEqual(stub.server.posts, [])
                self.assertFalse(path.exists())
                self.assertFalse(Path(str(path) + ".lock").exists())
        finally:
            stub.close()

    def test_rejection_reports_the_real_status_and_is_not_queued(self):
        stub = Stub(mode="status", status=401)
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                result = run_hook(
                    stub.url, rich_payload("denied"), path, CAIRN_DEVICE_TOKEN=DEVICE
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("rejected hook payload (401)", result.stderr)
                self.assertNotIn(QUEUED, result.stderr)
                self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET, PEM)
                self.assertFalse(path.exists())
        finally:
            stub.close()

    def test_bad_binding_fails_before_post_and_keeps_bytes(self):
        stub = Stub(mode="status", status=500)
        try:
            with tempfile.TemporaryDirectory() as directory:
                directory = Path(directory)
                version_one = {
                    "version": 1,
                    "events": [{"payload": {"eventId": "old", "content": "version-one-kept"}}],
                }
                for name in ("version1", "route", "origin", "token", "version3"):
                    with self.subTest(name=name):
                        stub.server.posts = []
                        path = directory / (name + ".json")
                        if name == "version1":
                            path.write_text(json.dumps(version_one) + "\n", encoding="utf-8")
                        elif name == "route":
                            self.write_bound(path, stub.url, "route-kept", route="observe")
                        elif name == "origin":
                            self.write_bound(path, "http://127.0.0.1:9", "origin-kept")
                        elif name == "token":
                            self.write_bound(path, stub.url, "token-kept", token=OTHER)
                        else:
                            self.write_bound(path, stub.url, "version-kept", version=3)
                        before = path.read_bytes()
                        result = run_hook(
                            stub.url,
                            rich_payload(name + "-event"),
                            path,
                            CAIRN_DEVICE_TOKEN=DEVICE,
                        )
                        self.assertNotEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(stub.server.posts, [])
                        self.assertEqual(path.read_bytes(), before)
                        self.assertNotIn(QUEUED, result.stderr)
                        self.assertNotIn("version-one-kept", result.stderr)
                        self.assert_hidden(result.stdout + result.stderr, DEVICE, OTHER, SECRET, PEM)
                        temps = [item.name for item in directory.iterdir() if item.name.endswith(".tmp")]
                        self.assertEqual(temps, [])
        finally:
            stub.close()

    def test_encoded_origin_is_refused_before_a_request(self):
        stub = Stub(mode="status", status=500)
        try:
            port = stub.server.server_address[1]
            urls = (
                "http://127.0.0.1%2eattacker.example:" + str(port),
                stub.url + "/cairn-memory",
                stub.url + "?token=query-secret",
                "http://userinfo-hook7b:pw@127.0.0.1:" + str(port),
            )
            for url in urls:
                with self.subTest(url=url.split("@")[-1][:24]):
                    stub.server.posts = []
                    with tempfile.TemporaryDirectory() as directory:
                        path = Path(directory) / "queue.json"
                        result = run_hook(url, rich_payload("bad-origin"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertLess(result.elapsed, 2)
                        self.assertEqual(stub.server.posts, [])
                        self.assertFalse(path.exists())
                        self.assertFalse(Path(str(path) + ".lock").exists())
                        self.assertIn("outbox origin must be an http(s) server base", result.stderr)
                        self.assert_hidden(
                            result.stdout + result.stderr,
                            DEVICE,
                            SECRET,
                            "attacker.example",
                            "userinfo-hook7b",
                            "query-secret",
                            "%2e",
                        )
        finally:
            stub.close()

    def test_full_queue_and_disk_error_preserve_bytes(self):
        stub = Stub(mode="status", status=500)
        try:
            with tempfile.TemporaryDirectory() as directory:
                directory = Path(directory)
                full = directory / "full.json"
                self.write_bound(full, stub.url, "seed")
                state = json.loads(full.read_text(encoding="utf-8"))
                state["events"] = [
                    {"payload": {"eventId": "full-%d" % index, "content": "kept"}, "attempts": 0, "lastError": None}
                    for index in range(256)
                ]
                full.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                before = full.read_bytes()
                result = run_hook(stub.url, rich_payload("overflow"), full, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("full", result.stderr)
                self.assertNotIn(QUEUED, result.stderr)
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(full.read_bytes(), before)
                self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET, PEM)

                stub.server.posts = []
                disk = directory / "disk.json"
                self.write_bound(disk, stub.url, "disk-kept")
                disk_before = disk.read_bytes()
                stub.server.chmod_dir = str(directory)
                try:
                    failed = run_hook(stub.url, rich_payload("disk-event"), disk, CAIRN_DEVICE_TOKEN=DEVICE)
                finally:
                    os.chmod(directory, 0o700)
                    stub.server.chmod_dir = None
                self.assertNotEqual(failed.returncode, 0)
                self.assertEqual(failed.stderr.strip(), "Cairn hook queue failed")
                self.assertNotIn(QUEUED, failed.stderr)
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(disk.read_bytes(), disk_before)
                temps = [item.name for item in directory.iterdir() if item.name.endswith(".tmp")]
                self.assertEqual(temps, [])
                self.assert_hidden(failed.stdout + failed.stderr, DEVICE, SECRET, PEM)
        finally:
            stub.close()

    def test_lock_held_by_another_process_returns_without_waiting(self):
        stub = Stub(mode="status", status=500)
        holder = None
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                lock = Path(str(path) + ".lock")
                holder = subprocess.Popen(
                    [
                        sys.executable,
                        "-c",
                        "import fcntl,os,sys,time\n"
                        "fd=os.open(sys.argv[1], os.O_CREAT|os.O_RDWR, 0o600)\n"
                        "fcntl.flock(fd, fcntl.LOCK_EX)\n"
                        "print('held', flush=True)\n"
                        "time.sleep(30)\n",
                        str(lock),
                    ],
                    stdout=subprocess.PIPE,
                    text=True,
                )
                line = holder.stdout.readline()
                self.assertEqual(line.strip(), "held")
                result = run_hook(
                    stub.url,
                    rich_payload("busy-event"),
                    path,
                    timeout=5,
                    CAIRN_DEVICE_TOKEN=DEVICE,
                )
                # A busy preflight is not fatal: the post goes out, and only the
                # retry append needs the lock, so a failed post is not retained.
                self.assertNotEqual(result.returncode, 0)
                self.assertLess(result.elapsed, 2)
                self.assertIn(NOT_RETAINED, result.stderr)
                self.assertNotIn(QUEUED, result.stderr)
                self.assertNotIn("no-drop", result.stderr)
                self.assertFalse(path.exists())
                self.assertEqual(len(stub.server.posts), 1)
                self.assert_hidden(result.stdout + result.stderr, DEVICE, SECRET)
                self.assert_no_argv_token(result)
        finally:
            if holder is not None:
                holder.kill()
                holder.wait(timeout=5)
                if holder.stdout is not None:
                    holder.stdout.close()
            stub.close()

    def test_concurrent_hooks_keep_every_successful_id(self):
        stub = Stub(mode="status", status=500)
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                processes = []
                for event_id in ("concurrent-a", "concurrent-b"):
                    child_env = os.environ.copy()
                    for name in ("CAIRN_ACCOUNT_TOKEN", "CAIRN_DEVICE_TOKEN", "CAIRN_OUTBOX"):
                        child_env.pop(name, None)
                    child_env["CAIRN_URL"] = stub.url
                    child_env["CAIRN_OUTBOX"] = str(path)
                    child_env["CAIRN_DEVICE_TOKEN"] = DEVICE
                    processes.append(
                        (
                            event_id,
                            subprocess.Popen(
                                [sys.executable, str(HOOK)],
                                stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE,
                                text=True,
                                env=child_env,
                                cwd=ROOT,
                            ),
                        )
                    )
                results = []
                for event_id, process in processes:
                    stdout, stderr = process.communicate(
                        json.dumps(rich_payload(event_id)),
                        timeout=20,
                    )
                    results.append((event_id, process.returncode, stdout, stderr, process.args))
                self.assertLessEqual(len(stub.server.posts), 2)
                stored = set()
                if path.exists():
                    state = json.loads(path.read_text(encoding="utf-8"))
                    stored = {item["payload"]["eventId"] for item in state["events"]}
                for event_id, code, stdout, stderr, args in results:
                    self.assertNotIn(DEVICE, args)
                    self.assert_hidden(stdout + stderr, DEVICE, SECRET, PEM)
                    if code == 0:
                        self.assertIn(event_id, stored)
                        self.assertIn(QUEUED, stderr)
                    else:
                        self.assertNotIn(QUEUED, stderr)
                        self.assertIn(NOT_RETAINED, stderr)
                self.assertTrue(stored or any(item[1] != 0 for item in results))
        finally:
            stub.close()

    def write_queue(self, path, origin, count, token=DEVICE):
        self.write_bound(path, origin, "q-0", token=token)
        state = json.loads(path.read_text(encoding="utf-8"))
        state["events"] = [
            {
                "payload": {"eventId": "q-%d" % number, "content": "kept", "sessionId": "s"},
                "attempts": 0,
                "lastError": None,
            }
            for number in range(count)
        ]
        path.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    def pending(self, path):
        return json.loads(path.read_text(encoding="utf-8"))["events"]

    def test_success_replays_oldest_twenty_then_the_rest(self):
        stub = Stub(mode="accept")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 25)
                result = run_hook(stub.url, rich_payload("own-1"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, '{"accepted":true,"duplicate":true}\n')
                self.assertEqual(result.stderr, "")
                self.assertEqual(len(stub.server.posts), 21)
                self.assertEqual(list(stub.server.rows), ["own-1"] + ["q-%d" % n for n in range(20)])
                self.assertEqual([i["payload"]["eventId"] for i in self.pending(path)], ["q-%d" % n for n in range(20, 25)])
                again = run_hook(stub.url, rich_payload("own-2"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(again.returncode, 0, again.stderr)
                self.assertEqual(self.pending(path), [])
                self.assertEqual(len(stub.server.rows), 27)
        finally:
            stub.close()

    def test_busy_lock_after_delivery_replays_nothing(self):
        stub = Stub(mode="accept")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 3)
                before = path.read_bytes()
                stub.server.hold_lock = str(path) + ".lock"
                result = run_hook(stub.url, rich_payload("own-1"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, '{"accepted":true,"duplicate":true}\n')
                self.assertEqual(result.stderr, "")
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(path.read_bytes(), before)
        finally:
            stub.close()

    def test_replay_transport_failure_keeps_the_hook_result_and_stops(self):
        stub = Stub(mode="accept_then_cut")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 3)
                result = run_hook(stub.url, rich_payload("own-1"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, '{"accepted":true,"duplicate":true}\n')
                self.assertEqual(result.stderr, "")
                # Own event plus one replay attempt; the first failure stops the pass.
                self.assertEqual(len(stub.server.posts), 2)
                events = self.pending(path)
                self.assertEqual([i["attempts"] for i in events], [1, 0, 0])
                self.assertTrue(events[0]["lastError"])
                self.assertIsNone(events[1]["lastError"])
        finally:
            stub.close()

    def test_replay_skips_the_hooks_own_event_id(self):
        stub = Stub(mode="accept")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 2)
                result = run_hook(stub.url, rich_payload("q-0"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(stub.server.posts), 2)
                self.assertEqual([i["payload"]["eventId"] for i in self.pending(path)], ["q-0"])
        finally:
            stub.close()

    def test_other_server_or_device_binding_is_never_sent(self):
        stub = Stub(mode="accept")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, "http://127.0.0.1:9", 2)
                before = path.read_bytes()
                result = run_hook(stub.url, rich_payload("own-1"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(stub.server.posts, [])
                self.assertEqual(path.read_bytes(), before)
                self.write_queue(path, stub.url, 2, token=OTHER)
                before = path.read_bytes()
                result = run_hook(stub.url, rich_payload("own-2"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(stub.server.posts, [])
                self.assertEqual(path.read_bytes(), before)
                # The replay entry point refuses the same mismatches before a socket.
                with self.assertRaises(ValueError):
                    OUTBOX.replay_bounded(path, stub.url, DEVICE, limit=20, budget=2.0)
                self.write_queue(path, "http://127.0.0.1:9", 2)
                with self.assertRaises(ValueError):
                    OUTBOX.replay_bounded(path, stub.url, DEVICE, limit=20, budget=2.0)
                self.assertEqual(stub.server.posts, [])
        finally:
            stub.close()

    def test_busy_lock_at_start_still_delivers_the_capture(self):
        stub = Stub(mode="accept")
        holder = None
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 2)
                before = path.read_bytes()
                holder = subprocess.Popen(
                    [
                        sys.executable,
                        "-c",
                        "import fcntl,os,sys,time\n"
                        "fd=os.open(sys.argv[1], os.O_CREAT|os.O_RDWR, 0o600)\n"
                        "fcntl.flock(fd, fcntl.LOCK_EX)\n"
                        "print('held', flush=True)\n"
                        "time.sleep(30)\n",
                        str(path) + ".lock",
                    ],
                    stdout=subprocess.PIPE,
                    text=True,
                )
                self.assertEqual(holder.stdout.readline().strip(), "held")
                result = run_hook(stub.url, rich_payload("own-1"), path, timeout=5, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, '{"accepted":true,"duplicate":true}\n')
                self.assertEqual(result.stderr, "")
                self.assertEqual(len(stub.server.posts), 1)
                self.assertEqual(path.read_bytes(), before)
        finally:
            if holder is not None:
                holder.kill()
                holder.wait(timeout=5)
                if holder.stdout is not None:
                    holder.stdout.close()
            stub.close()

    def test_replay_401_stops_without_purging(self):
        stub = Stub(mode="accept_then_401")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 3)
                result = run_hook(stub.url, rich_payload("own-1"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stderr, "")
                self.assertEqual(len(stub.server.posts), 2)
                events = self.pending(path)
                self.assertEqual([i["attempts"] for i in events], [1, 0, 0])
                self.assertIn("401", events[0]["lastError"])
        finally:
            stub.close()

    def test_exhausted_events_do_not_starve_newer_ones(self):
        stub = Stub(mode="accept")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 25)
                state = json.loads(path.read_text(encoding="utf-8"))
                for item in state["events"][:2]:
                    item["attempts"] = 3
                path.write_text(json.dumps(state), encoding="utf-8")
                result = run_hook(stub.url, rich_payload("own-1"), path, CAIRN_DEVICE_TOKEN=DEVICE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(list(stub.server.rows), ["own-1"] + ["q-%d" % n for n in range(2, 22)])
                self.assertEqual(
                    [i["payload"]["eventId"] for i in self.pending(path)],
                    ["q-0", "q-1", "q-22", "q-23", "q-24"],
                )
                self.assertEqual([i["attempts"] for i in self.pending(path)][:2], [3, 3])
        finally:
            stub.close()

    def test_budget_bounds_a_stalled_replay_request(self):
        stub = Stub(mode="stall")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 3)
                started = time.monotonic()
                OUTBOX.replay_bounded(path, stub.url, DEVICE, limit=20, budget=0.5)
                self.assertLess(time.monotonic() - started, 2)
                self.assertEqual([i["attempts"] for i in self.pending(path)], [1, 0, 0])
        finally:
            stub.close()

    def test_spent_budget_attempts_nothing(self):
        stub = Stub(mode="accept")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                self.write_queue(path, stub.url, 2)
                OUTBOX.replay_bounded(path, stub.url, DEVICE, limit=20, budget=0.0)
                self.assertEqual(stub.server.posts, [])
                self.assertEqual([i["attempts"] for i in self.pending(path)], [0, 0])
        finally:
            stub.close()

    def write_bound(self, path, origin, event_id, token=DEVICE, route="sync/push", version=2):
        state = {
            "version": version,
            "route": route,
            "origin": OUTBOX.normalize_origin(origin) if "://" in origin else origin,
            "deviceFingerprint": OUTBOX.device_fingerprint(token),
            "events": [
                {
                    "payload": {"eventId": event_id, "content": "kept", "sessionId": "s"},
                    "attempts": 0,
                    "lastError": None,
                }
            ],
        }
        path.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return path.read_bytes()


if __name__ == "__main__":
    unittest.main()
