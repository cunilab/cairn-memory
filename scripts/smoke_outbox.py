#!/usr/bin/env python3
"""Narrow spool check: content and context-list redaction, queue policy, and replay.

Synthetic tokens only. Reads the queue file bytes, not only the helper return
value. Does not start the Cairn server or touch a live store.
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from contextlib import redirect_stdout
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUTBOX_PATH = ROOT / "scripts" / "cairn_outbox.py"
SK = "SK-synthetic-example"
BODY = "c3ludGhldGljLWtleS1ib2R5"
IV = "SYNTHETICIVVALUE"
MARKER_VALUES = (
    "synthetic-bearer-value",
    "synthetic-api-value",
    "synthetic-token-value",
    "synthetic-pass-value",
)
PRIOR_SK = "SK-prior-spool-example"
CERT = "bm90LWEtY2VydA=="
PUBLIC_KEY = "bm90LWEtcHVibGljLWtleQ=="
PROSE = "The armor label is PRIVATE KEY in prose."
TAIL = "ordinary sentence stays"
NOTE = "tail note stays"


def load_outbox():
    spec = importlib.util.spec_from_file_location("cairn_outbox", OUTBOX_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load cairn_outbox.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


OUTBOX = load_outbox()


def enqueue_in_process(path: Path, event: dict) -> str:
    buffer = io.StringIO()
    original = sys.stdin
    sys.stdin = io.StringIO(json.dumps(event))
    try:
        with redirect_stdout(buffer):
            code = OUTBOX.enqueue(path)
    finally:
        sys.stdin = original
    if code != 0:
        raise AssertionError(f"enqueue returned {code}")
    return buffer.getvalue()


def enqueue_cli(path: Path, event: dict) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(OUTBOX_PATH), "enqueue", str(path)],
        input=json.dumps(event),
        text=True,
        capture_output=True,
        check=False,
    )


class _PushHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        server = self.server
        server.bodies.append(body)  # type: ignore[attr-defined]
        code = 200
        codes = server.codes  # type: ignore[attr-defined]
        if codes:
            code = codes.pop(0)
        if code != 200:
            self.send_response(code)
            self.end_headers()
            return
        payload = b'{"accepted": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format: str, *args) -> None:
        return


def serve(codes: list[int]) -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _PushHandler)
    server.bodies = []  # type: ignore[attr-defined]
    server.codes = codes  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address[:2]
    return server, f"http://{host}:{port}"


class _RedirectHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self._record("GET")

    def do_POST(self) -> None:
        self._record("POST")

    def _record(self, method: str) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        server = self.server
        server.hits.append(  # type: ignore[attr-defined]
            {
                "method": method,
                "path": self.path.split("?", 1)[0],
                "authorization": self.headers.get("Authorization"),
                "body": body,
            }
        )
        code = server.reply_code  # type: ignore[attr-defined]
        payload = server.reply_body  # type: ignore[attr-defined]
        self.send_response(code)
        location = server.location  # type: ignore[attr-defined]
        if location:
            self.send_header("Location", location)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format: str, *args) -> None:
        return


def serve_reply(
    code: int, body: bytes, location: str | None = None
) -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _RedirectHandler)
    server.hits = []  # type: ignore[attr-defined]
    server.reply_code = code  # type: ignore[attr-defined]
    server.reply_body = body  # type: ignore[attr-defined]
    server.location = location  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address[:2]
    return server, f"http://{host}:{port}"


class OutboxRedactionCheck(unittest.TestCase):
    def test_marker_followed_by_space_or_end_is_still_marked(self) -> None:
        # Core redact_after_marker inserts the marker even with an empty value.
        self.assertEqual(OUTBOX.redact_content("token= x"), "token=[REDACTED] x")
        self.assertEqual(OUTBOX.redact_content("a password="), "a password=[REDACTED]")

    def test_queue_file_bytes_omit_synthetic_content_secrets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "outbox.json"
            blocks = [
                "before\r\n-----BEGIN RSA PRIVATE KEY-----\r\n"
                f"{BODY}\r\n-----END RSA PRIVATE KEY-----\r\nafter",
                "-----Begin Ec Private Key-----\n"
                f"{BODY}\n-----End Ec Private Key-----\n",
                "-----BEGIN ENCRYPTED PRIVATE KEY-----\n"
                f"{BODY}\n-----END ENCRYPTED PRIVATE KEY-----\n",
                "-----BEGIN PRIVATE KEY-----\n"
                f"{BODY}\n-----END PRIVATE KEY-----\n",
                "lead -----BEGIN EC PRIVATE KEY----- "
                f"{BODY}",
                "-----BEGIN RSA PRIVATE KEY-----\n"
                "Proc-Type: 4,ENCRYPTED\n"
                f"DEK-Info: AES-128-CBC,{IV}\n"
                "\n"
                f"{BODY}\n{TAIL}\n",
                "notes before\n-----BEGIN OPENSSH PRIVATE KEY-----\n"
                f"{BODY}\n{NOTE}\n",
                "first -----BEGIN RSA PRIVATE KEY-----\n"
                f"{BODY}\n-----END RSA PRIVATE KEY-----\n"
                "prose between\n"
                "-----BEGIN EC PRIVATE KEY-----\n"
                f"{BODY}\n-----END EC PRIVATE KEY-----\n",
            ]
            content = "\n".join(
                [
                    f"caf\u00e9 menu {SK} \u043a\u043e\u043d\u0435\u0446",
                    "sk-synthetic-example",
                    "Sk-synthetic-example",
                    "key=(sk-synthetic-example) disk-full, risk-based task-queue",
                    "line\r\n" + SK + "\r\nnext",
                    "Authorization: Bearer synthetic-bearer-value "
                    "api_key=synthetic-api-value token=synthetic-token-value "
                    "password=synthetic-pass-value",
                    "Password=synthetic-pass-value",
                    PROSE,
                    f"-----BEGIN CERTIFICATE-----\n{CERT}\n-----END CERTIFICATE-----\n",
                    f"-----BEGIN PUBLIC KEY-----\n{PUBLIC_KEY}\n-----END PUBLIC KEY-----\n",
                    *blocks,
                ]
            )
            event = {
                "eventId": "evt-outbox-redact",
                "sessionId": "sess-outbox-redact",
                "role": "agent",
                "source": "outbox-check",
                "context": {"repository": "cairn-demo", "branch": "main"},
                "content": content,
            }
            completed = enqueue_cli(path, event)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            raw = path.read_bytes()
            for needle in (SK, "sk-synthetic-example", "Sk-synthetic-example", BODY, IV, *MARKER_VALUES):
                self.assertNotIn(needle.encode(), raw, needle)
            text = raw.decode()
            self.assertIn("evt-outbox-redact", text)
            self.assertIn("sess-outbox-redact", text)
            self.assertIn("cairn-demo", text)
            self.assertIn("[REDACTED]", text)
            self.assertIn("Password=[REDACTED]", text)
            self.assertIn("key=([REDACTED] disk-full, risk-based task-queue", text)
            self.assertIn("Authorization: Bearer [REDACTED]", text)
            stored = json.loads(raw)
            stored_content = stored["events"][0]["payload"]["content"]
            self.assertIn(PROSE, stored_content)
            self.assertIn(CERT, stored_content)
            self.assertIn(PUBLIC_KEY, stored_content)
            self.assertIn(TAIL, stored_content)
            self.assertIn(NOTE, stored_content)
            self.assertIn("prose between", stored_content)
            self.assertIn("caf\u00e9 menu ", stored_content)
            self.assertIn(" \u043a\u043e\u043d\u0435\u0446", stored_content)
            self.assertIn("before", stored_content)
            self.assertIn("after", stored_content)
            self.assertNotIn("BEGIN RSA PRIVATE KEY", stored_content)
            self.assertNotIn("BEGIN OPENSSH PRIVATE KEY", stored_content)
            self.assertEqual(stored["events"][0]["payload"]["eventId"], "evt-outbox-redact")
            self.assertEqual(stored["events"][0]["attempts"], 0)

    def test_truncated_footer_drops_glued_body_and_old_spool_stays(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            residual = Path(directory) / "residual.json"
            glued = (
                "-----BEGIN RSA PRIVATE KEY-----\n"
                f"{BODY}-----END RSA PRIVATE KEY\n"
                "next prose stays\n"
            )
            completed = enqueue_cli(
                residual,
                {"eventId": "evt-residual", "content": glued},
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            raw = residual.read_bytes()
            self.assertNotIn(BODY.encode(), raw)
            self.assertNotIn(b"-----END RSA PRIVATE KEY", raw)
            self.assertIn(b"next prose stays", raw)
            stored = json.loads(raw)
            content = stored["events"][0]["payload"]["content"]
            self.assertEqual(content, "[REDACTED]\n\nnext prose stays\n")
            self.assertEqual(stored["events"][0]["payload"]["eventId"], "evt-residual")

            server, base = serve([])
            try:
                replayed = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(residual),
                        "--url",
                        base,
                        "--token",
                        "synthetic-device-token",
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
            finally:
                server.shutdown()
                server.server_close()
            self.assertEqual(replayed.returncode, 0, replayed.stderr)
            posted = json.loads(server.bodies[0])  # type: ignore[attr-defined]
            self.assertNotIn(BODY, posted["content"])
            self.assertIn("next prose stays", posted["content"])
            self.assertEqual(json.loads(residual.read_text(encoding="utf-8"))["events"], [])

            suffix_path = Path(directory) / "suffix.json"
            enqueue_in_process(
                suffix_path,
                {
                    "eventId": "evt-suffix",
                    "content": (
                        "-----BEGIN RSA PRIVATE KEY-----\n"
                        f"{BODY}-----END RSA PRIVATE KEY then the note stays\n"
                    ),
                },
            )
            suffix_raw = suffix_path.read_bytes()
            self.assertNotIn(BODY.encode(), suffix_raw)
            self.assertIn(b"then the note stays", suffix_raw)
            self.assertEqual(
                json.loads(suffix_raw)["events"][0]["payload"]["content"],
                "[REDACTED]\n then the note stays\n",
            )

            mixed_path = Path(directory) / "mixed.json"
            enqueue_in_process(
                mixed_path,
                {
                    "eventId": "evt-mixed-footer",
                    "content": (
                        "-----Begin Rsa Private Key-----\r\n"
                        f"{BODY}-----End Rsa Private Key\r\n"
                        "after glue\r\n"
                    ),
                },
            )
            mixed = json.loads(mixed_path.read_text(encoding="utf-8"))
            mixed_content = mixed["events"][0]["payload"]["content"]
            self.assertNotIn(BODY, mixed_content)
            self.assertIn("after glue", mixed_content)
            self.assertNotIn("End Rsa Private Key", mixed_content)

            complete_path = Path(directory) / "complete.json"
            enqueue_in_process(
                complete_path,
                {
                    "eventId": "evt-complete",
                    "content": (
                        "-----BEGIN EC PRIVATE KEY-----\n"
                        f"{BODY}-----END EC PRIVATE KEY-----\n"
                        "kept complete\n"
                    ),
                },
            )
            complete = json.loads(complete_path.read_text(encoding="utf-8"))
            complete_content = complete["events"][0]["payload"]["content"]
            self.assertNotIn(BODY, complete_content)
            self.assertIn("kept complete", complete_content)

            no_end_path = Path(directory) / "no-end.json"
            enqueue_in_process(
                no_end_path,
                {
                    "eventId": "evt-no-end",
                    "content": (
                        "-----BEGIN OPENSSH PRIVATE KEY-----\n"
                        f"{BODY}\n"
                        "tail note stays\n"
                    ),
                },
            )
            no_end = json.loads(no_end_path.read_text(encoding="utf-8"))
            no_end_content = no_end["events"][0]["payload"]["content"]
            self.assertNotIn(BODY, no_end_content)
            self.assertIn("tail note stays", no_end_content)

            prose_path = Path(directory) / "prose.json"
            enqueue_in_process(
                prose_path,
                {
                    "eventId": "evt-prose-key",
                    "content": (
                        "-----BEGIN RSA PRIVATE KEY-----\n"
                        "The armor label is PRIVATE KEY in prose.\n"
                    ),
                },
            )
            prose = json.loads(prose_path.read_text(encoding="utf-8"))
            self.assertIn(
                "The armor label is PRIVATE KEY in prose.",
                prose["events"][0]["payload"]["content"],
            )

            unrecognized = Path(directory) / "unrecognized.json"
            header = f"-----BEGIN RSA PRIVATE KEY\n{BODY}\n"
            enqueue_in_process(
                unrecognized,
                {"eventId": "evt-open-header", "content": header},
            )
            open_header = json.loads(unrecognized.read_text(encoding="utf-8"))
            self.assertEqual(
                open_header["events"][0]["payload"]["content"],
                header,
            )

            dsa = Path(directory) / "dsa.json"
            dsa_block = (
                "-----BEGIN DSA PRIVATE KEY-----\n"
                f"{BODY}\n-----END DSA PRIVATE KEY-----\n"
            )
            enqueue_in_process(
                dsa,
                {"eventId": "evt-dsa", "content": dsa_block},
            )
            self.assertEqual(
                json.loads(dsa.read_text(encoding="utf-8"))["events"][0]["payload"]["content"],
                dsa_block,
            )

            prior = Path(directory) / "prior.json"
            prior.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "events": [
                            {
                                "payload": {
                                    "eventId": "evt-prior",
                                    "content": PRIOR_SK,
                                },
                                "attempts": 0,
                                "lastError": None,
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            enqueue_in_process(
                prior,
                {
                    "eventId": "evt-new",
                    "content": f"kept note {SK}",
                },
            )
            raw = prior.read_bytes()
            self.assertIn(PRIOR_SK.encode(), raw)
            self.assertNotIn(SK.encode(), raw)
            loaded = json.loads(raw)
            self.assertEqual(loaded["events"][0]["payload"]["content"], PRIOR_SK)
            self.assertIn("kept note ", loaded["events"][1]["payload"]["content"])

    def test_queue_policy_replay_order_and_auth_purge(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            first = enqueue_in_process(
                path,
                {
                    "eventId": "evt-order-1",
                    "sessionId": "sess-replay",
                    "content": f"alpha note {SK}",
                },
            )
            self.assertIn('"queued": true', first)
            duplicate = enqueue_in_process(
                path,
                {
                    "eventId": "evt-order-1",
                    "content": f"replacement {BODY}",
                },
            )
            self.assertIn('"duplicate": true', duplicate)
            raw = path.read_bytes()
            self.assertNotIn(SK.encode(), raw)
            self.assertNotIn(BODY.encode(), raw)
            self.assertEqual(len(json.loads(raw)["events"]), 1)

            second = enqueue_in_process(
                path,
                {
                    "eventId": "evt-order-2",
                    "sessionId": "sess-replay",
                    "content": "beta note",
                },
            )
            self.assertIn('"queued": true', second)

            server, base = serve([])
            try:
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(path),
                        "--url",
                        base,
                        "--token",
                        "synthetic-device-token",
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
            finally:
                server.shutdown()
                server.server_close()
            self.assertEqual(completed.returncode, 0, completed.stderr)
            report = json.loads(completed.stdout)
            self.assertEqual(report["accepted"], 2)
            self.assertEqual(report["pending"], 0)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["events"], [])
            posted = [json.loads(body) for body in server.bodies]  # type: ignore[attr-defined]
            self.assertEqual([item["eventId"] for item in posted], ["evt-order-1", "evt-order-2"])
            self.assertEqual(posted[0]["sessionId"], "sess-replay")
            self.assertIn("alpha note ", posted[0]["content"])
            self.assertNotIn(SK, posted[0]["content"])
            self.assertEqual(posted[1]["content"], "beta note")
            self.assertNotIn(b"synthetic-device-token", path.read_bytes())

            revoked = Path(directory) / "revoked.json"
            enqueue_in_process(revoked, {"eventId": "evt-revoke-1", "content": "one"})
            enqueue_in_process(revoked, {"eventId": "evt-revoke-2", "content": f"two {SK}"})
            self.assertNotIn(SK.encode(), revoked.read_bytes())
            deny, deny_base = serve([401])
            try:
                denied = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(revoked),
                        "--url",
                        deny_base,
                        "--token",
                        "synthetic-revoked-token",
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
            finally:
                deny.shutdown()
                deny.server_close()
            self.assertNotEqual(denied.returncode, 0)
            denied_report = json.loads(denied.stdout)
            self.assertEqual(denied_report["purged"], 2)
            self.assertEqual(denied_report["pending"], 0)
            self.assertEqual(json.loads(revoked.read_text(encoding="utf-8"))["events"], [])

            full = Path(directory) / "full.json"
            for index in range(OUTBOX.MAX_EVENTS):
                enqueue_in_process(
                    full,
                    {"eventId": f"evt-fill-{index}", "content": "queue-fill"},
                )
            with self.assertRaises(ValueError):
                enqueue_in_process(
                    full,
                    {"eventId": "evt-overflow", "content": f"overflow {SK}"},
                )
            full_raw = full.read_bytes()
            self.assertNotIn(b"evt-overflow", full_raw)
            self.assertNotIn(SK.encode(), full_raw)
            self.assertEqual(len(json.loads(full_raw)["events"]), OUTBOX.MAX_EVENTS)

            limited = Path(directory) / "limited.json"
            with self.assertRaises(ValueError):
                enqueue_in_process(
                    limited,
                    {
                        "eventId": "evt-oversize",
                        "content": "x" * (OUTBOX.MAX_CONTENT_BYTES + 1),
                    },
                )
            self.assertFalse(limited.exists())

    def test_conflict_is_dropped_from_the_queue_and_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "conflict.json"
            enqueue_in_process(path, {"eventId": "evt-conflict", "content": "changed text"})
            enqueue_in_process(path, {"eventId": "evt-fine", "content": "fine text"})
            server, base = serve([409, 200])
            try:
                done = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(path),
                        "--url",
                        base,
                        "--token",
                        "synthetic-device-token",
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
            finally:
                server.shutdown()
                server.server_close()
            self.assertEqual(done.returncode, 1, done.stderr)
            report = json.loads(done.stdout)
            self.assertEqual(
                (report["accepted"], report["failed"], report["pending"]), (1, 0, 0)
            )
            self.assertEqual([item["eventId"] for item in report["rejected"]], ["evt-conflict"])
            self.assertIn("409", report["rejected"][0]["reason"])
            state = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(state["events"], [])
            self.assertEqual(state["rejectedCount"], 1)
            self.assertEqual(state["lastRejected"]["eventId"], "evt-conflict")

            status = subprocess.run(
                [sys.executable, str(OUTBOX_PATH), "status", str(path)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(json.loads(status.stdout)["rejectedCount"], 1)

    def test_bounded_replay_drops_a_conflict_silently_and_counts_it(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bounded.json"
            server, base = serve([409, 200])
            try:
                for event_id in ("evt-conflict", "evt-fine"):
                    OUTBOX.enqueue_bound(
                        path, {"eventId": event_id, "content": event_id}, base, "synthetic-device-token", report=False
                    )
                captured = io.StringIO()
                with redirect_stdout(captured):
                    OUTBOX.replay_bounded(
                        path, base, "synthetic-device-token", limit=20, budget=2.0
                    )
            finally:
                server.shutdown()
                server.server_close()
            self.assertEqual(captured.getvalue(), "")
            state = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(state["events"], [])
            self.assertEqual(state["rejectedCount"], 1)
            self.assertEqual(state["lastRejected"]["eventId"], "evt-conflict")
            self.assertIn("409", state["lastRejected"]["reason"])

    def test_context_lists_are_redacted_before_the_spool_write(self) -> None:
        lists = (
            "files",
            "symbols",
            "commands",
            "tests",
            "services",
            "handoffs",
            "runbooks",
        )
        blocks = {
            "files": (
                "-----BEGIN RSA PRIVATE KEY-----\n"
                f"{BODY}\n-----END RSA PRIVATE KEY-----\n"
            ),
            "symbols": (
                "-----BEGIN EC PRIVATE KEY-----\n"
                f"{BODY}\n-----END EC PRIVATE KEY-----\n"
            ),
            "commands": (
                "-----BEGIN OPENSSH PRIVATE KEY-----\n"
                f"{BODY}\n-----END OPENSSH PRIVATE KEY-----\n"
            ),
            "tests": (
                "-----BEGIN ENCRYPTED PRIVATE KEY-----\n"
                f"{BODY}\n-----END ENCRYPTED PRIVATE KEY-----\n"
            ),
            "services": (
                "-----BEGIN PRIVATE KEY-----\n"
                f"{BODY}\n-----END PRIVATE KEY-----\n"
            ),
            "handoffs": (
                "-----BEGIN RSA PRIVATE KEY-----\n"
                f"{BODY}\n-----END RSA PRIVATE KEY-----\n"
            ),
            "runbooks": (
                "-----Begin Ec Private Key-----\n"
                f"{BODY}\n-----End Ec Private Key-----\n"
            ),
        }
        ordinary = {
            "files": "src/lib.rs",
            "symbols": "MemoryStore::open",
            "commands": "cargo test cache",
            "tests": "cache_reads_are_consistent",
            "services": "cairn",
            "handoffs": "handoff stays caf\u00e9",
            "runbooks": "runbook stays \u043a\u043e\u043d\u0435\u0446",
        }
        shared_files = [f"shared {SK}", "caf\u00e9.rs"]
        source = {
            "event_id": "evt-alias",
            "sessionId": "sess-alias",
            "personId": "person-alias",
            "capturedAt": "2026-11-12T00:00:00Z",
            "cursor": 7,
            "context": {
                "repository": "cairn-demo",
                "branch": "sk-looking-name",
                "worktree": "wt-main",
                "commit": "abc1234",
                "files": shared_files,
            },
        }
        normalized = OUTBOX.normalize(source)
        self.assertIs(source["context"]["files"], shared_files)
        self.assertEqual(shared_files, [f"shared {SK}", "caf\u00e9.rs"])
        self.assertEqual(normalized["eventId"], "evt-alias")
        self.assertEqual(normalized["event_id"], "evt-alias")
        self.assertEqual(normalized["personId"], "person-alias")
        self.assertEqual(normalized["cursor"], 7)
        self.assertEqual(normalized["context"]["branch"], "sk-looking-name")
        self.assertEqual(normalized["context"]["commit"], "abc1234")
        self.assertIsNot(normalized["context"], source["context"])
        self.assertIsNot(normalized["context"]["files"], shared_files)
        self.assertEqual(normalized["context"]["files"][1], "caf\u00e9.rs")
        self.assertNotIn(SK, normalized["context"]["files"][0])
        self.assertEqual(
            OUTBOX.normalize({"eventId": "evt-missing-context", "content": "plain"}),
            {"eventId": "evt-missing-context", "content": "plain"},
        )
        self.assertIsNone(
            OUTBOX.normalize({"eventId": "evt-null-context", "context": None})["context"]
        )
        with self.assertRaises(ValueError) as malformed:
            OUTBOX.normalize(
                {"eventId": "evt-malformed-context", "context": f"raw {SK}"}
            )
        self.assertEqual(str(malformed.exception), "outbox context must be a JSON object")
        self.assertNotIn(SK, str(malformed.exception))
        with self.assertRaises(ValueError) as listed:
            OUTBOX.normalize(
                {"eventId": "evt-list-context", "context": [SK]}
            )
        self.assertEqual(str(listed.exception), "outbox context must be a JSON object")
        self.assertNotIn(SK, str(listed.exception))
        caller_symbols = [f"name {SK}", None]
        with self.assertRaises(ValueError) as wrong_type:
            OUTBOX.normalize(
                {
                    "eventId": "evt-wrong-type",
                    "context": {"files": SK, "symbols": caller_symbols},
                }
            )
        self.assertEqual(
            str(wrong_type.exception),
            "outbox context files must be a list of strings",
        )
        self.assertNotIn(SK, str(wrong_type.exception))
        self.assertEqual(caller_symbols, [f"name {SK}", None])
        caller_files = ["ordinary.rs", {"path": SK}]
        with self.assertRaises(ValueError) as dict_item:
            OUTBOX.normalize(
                {"eventId": "evt-dict-item", "context": {"files": caller_files}}
            )
        self.assertEqual(
            str(dict_item.exception),
            "outbox context files must be a list of strings",
        )
        self.assertNotIn(SK, str(dict_item.exception))
        self.assertEqual(caller_files[1], {"path": SK})
        nested_files = [[SK]]
        with self.assertRaises(ValueError) as nested:
            OUTBOX.normalize(
                {"eventId": "evt-nested-list", "context": {"files": nested_files}}
            )
        self.assertEqual(
            str(nested.exception),
            "outbox context files must be a list of strings",
        )
        self.assertEqual(nested_files, [[SK]])
        with self.assertRaises(ValueError) as null_list:
            OUTBOX.normalize(
                {"eventId": "evt-null-list", "context": {"symbols": None}}
            )
        self.assertEqual(
            str(null_list.exception),
            "outbox context symbols must be a list of strings",
        )

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "context.json"
            context = {
                "repository": "cairn-demo",
                "branch": "sk-looking-name",
                "worktree": "wt-main",
                "commit": "abc1234",
            }
            for name in lists:
                context[name] = [ordinary[name], f"note {SK}", blocks[name]]
            context["commands"].append("token=synthetic-token-value")
            event = {
                "eventId": "evt-context-redact",
                "sessionId": "sess-context-redact",
                "personId": "person-context-redact",
                "capturedAt": "2026-11-12T00:00:00Z",
                "cursor": 7,
                "role": "agent",
                "source": "outbox-check",
                "lifecycle": {"sensitivity": "public"},
                "content": "ordinary observation text",
                "context": context,
            }
            completed = enqueue_cli(path, event)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            raw = path.read_bytes()
            self.assertNotIn(SK.encode(), raw)
            self.assertNotIn(BODY.encode(), raw)
            self.assertNotIn(b"BEGIN RSA PRIVATE KEY", raw)
            self.assertNotIn(b"BEGIN OPENSSH PRIVATE KEY", raw)
            self.assertNotIn(b"synthetic-token-value", raw)
            text = raw.decode()
            self.assertIn("evt-context-redact", text)
            self.assertIn("sess-context-redact", text)
            self.assertIn("person-context-redact", text)
            self.assertIn("2026-11-12T00:00:00Z", text)
            self.assertIn("sk-looking-name", text)
            self.assertIn("wt-main", text)
            self.assertIn("abc1234", text)
            self.assertIn("cairn-demo", text)
            self.assertIn("ordinary observation text", text)
            self.assertIn('"sensitivity": "public"', text)
            stored = json.loads(raw)
            stored_context = stored["events"][0]["payload"]["context"]
            self.assertEqual(
                set(stored_context),
                {"repository", "branch", "worktree", "commit", *lists},
            )
            for name in lists:
                self.assertEqual(stored_context[name][0], ordinary[name])
                self.assertEqual(stored_context[name][1], "note [REDACTED]")
                self.assertNotIn(BODY, stored_context[name][2])
                self.assertNotIn(SK, stored_context[name][2])
            self.assertIn("token=[REDACTED]", stored_context["commands"][3])
            self.assertEqual(stored["events"][0]["payload"]["lifecycle"]["sensitivity"], "public")
            self.assertEqual(stored["events"][0]["payload"]["cursor"], 7)

            second = enqueue_in_process(
                path,
                {
                    "eventId": "evt-context-next",
                    "sessionId": "sess-context-redact",
                    "content": "beta note",
                    "context": {"files": [f"next {SK}", "src/keep.rs"]},
                },
            )
            self.assertIn('"queued": true', second)
            server, base = serve([])
            try:
                replayed = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(path),
                        "--url",
                        base,
                        "--token",
                        "synthetic-device-token",
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
            finally:
                server.shutdown()
                server.server_close()
            self.assertEqual(replayed.returncode, 0, replayed.stderr)
            posted = [json.loads(body) for body in server.bodies]  # type: ignore[attr-defined]
            self.assertEqual(
                [item["eventId"] for item in posted],
                ["evt-context-redact", "evt-context-next"],
            )
            self.assertEqual(posted[0]["context"]["branch"], "sk-looking-name")
            self.assertEqual(posted[0]["lifecycle"]["sensitivity"], "public")
            self.assertEqual(posted[1]["context"]["files"][0], "next [REDACTED]")
            self.assertEqual(posted[1]["context"]["files"][1], "src/keep.rs")
            self.assertEqual(posted[1]["content"], "beta note")
            posted_raw = b"".join(server.bodies)  # type: ignore[attr-defined]
            self.assertNotIn(SK.encode(), posted_raw)
            self.assertNotIn(BODY.encode(), posted_raw)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["events"], [])

            prior = Path(directory) / "prior-context.json"
            prior.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "events": [
                            {
                                "payload": {
                                    "eventId": "evt-prior-context",
                                    "context": {"files": [PRIOR_SK]},
                                },
                                "attempts": 0,
                                "lastError": None,
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            enqueue_in_process(
                prior,
                {
                    "eventId": "evt-after-prior",
                    "context": {"symbols": [SK]},
                },
            )
            prior_raw = prior.read_bytes()
            self.assertIn(PRIOR_SK.encode(), prior_raw)
            self.assertNotIn(SK.encode(), prior_raw)
            prior_loaded = json.loads(prior_raw)
            self.assertEqual(
                prior_loaded["events"][0]["payload"]["context"]["files"],
                [PRIOR_SK],
            )
            self.assertEqual(
                prior_loaded["events"][1]["payload"]["context"]["symbols"],
                ["[REDACTED]"],
            )

            preserved = Path(directory) / "preserved.json"
            preserved.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "events": [
                            {
                                "payload": {
                                    "eventId": "evt-kept",
                                    "content": "kept ordinary",
                                    "context": {"files": ["src/kept.rs"]},
                                },
                                "attempts": 0,
                                "lastError": None,
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            preserved_bytes = preserved.read_bytes()
            rejected = (
                (
                    {"eventId": "evt-string-context", "context": f"raw {SK}"},
                    "outbox context must be a JSON object",
                ),
                (
                    {"eventId": "evt-list-context", "context": [SK]},
                    "outbox context must be a JSON object",
                ),
                (
                    {
                        "eventId": "evt-scalar-files",
                        "context": {"files": SK, "symbols": ["kept ordinary"]},
                    },
                    "outbox context files must be a list of strings",
                ),
                (
                    {
                        "eventId": "evt-dict-item",
                        "context": {"files": ["ordinary.rs", {"path": SK}]},
                    },
                    "outbox context files must be a list of strings",
                ),
                (
                    {
                        "eventId": "evt-nested-list",
                        "context": {"files": [[SK]]},
                    },
                    "outbox context files must be a list of strings",
                ),
                (
                    {"eventId": "evt-null-list", "context": {"commands": None}},
                    "outbox context commands must be a list of strings",
                ),
            )
            for event, message in rejected:
                failed = enqueue_cli(preserved, event)
                self.assertNotEqual(failed.returncode, 0, message)
                failure_text = failed.stdout + failed.stderr
                self.assertIn(message, failure_text)
                self.assertNotIn(SK, failure_text)
                self.assertNotIn(BODY, failure_text)
                self.assertEqual(preserved.read_bytes(), preserved_bytes)
            self.assertEqual(
                json.loads(preserved_bytes)["events"][0]["payload"]["eventId"],
                "evt-kept",
            )
            self.assertEqual(
                json.loads(preserved_bytes)["events"][0]["payload"]["context"]["files"],
                ["src/kept.rs"],
            )

            fresh = Path(directory) / "rejected-new.json"
            fresh_fail = enqueue_cli(
                fresh,
                {"eventId": "evt-fresh-reject", "context": {"handoffs": f"note {SK}"}},
            )
            self.assertNotEqual(fresh_fail.returncode, 0)
            fresh_text = fresh_fail.stdout + fresh_fail.stderr
            self.assertIn("outbox context handoffs must be a list of strings", fresh_text)
            self.assertNotIn(SK, fresh_text)
            self.assertFalse(fresh.exists())

            missing = Path(directory) / "missing.json"
            failed = enqueue_cli(
                missing,
                {
                    "content": f"hidden {SK}",
                    "context": {"files": [SK, blocks["files"]]},
                },
            )
            self.assertNotEqual(failed.returncode, 0)
            failure_text = failed.stdout + failed.stderr
            self.assertNotIn(SK, failure_text)
            self.assertNotIn(BODY, failure_text)
            self.assertFalse(missing.exists())


class OutboxRedirectCheck(unittest.TestCase):
    def test_replay_does_not_follow_redirects_or_forward_the_bearer(self) -> None:
        token = "synthetic-device-token"
        location_marker = "synthetic-location-secret"
        body_marker = "synthetic-redirect-body"
        for code in (301, 302, 303, 307, 308):
            with self.subTest(code=code):
                self._replay_stops_at_origin(code, token, location_marker, body_marker)

    def _replay_stops_at_origin(
        self, code: int, token: str, location_marker: str, body_marker: str
    ) -> None:
        redirect_body = json.dumps(
            {"content": body_marker, "token": token, "location": location_marker}
        ).encode()
        target, target_base = serve_reply(200, b'{"accepted": true}')
        location = f"{target_base}/taken?token={token}&content={location_marker}"
        origin, origin_base = serve_reply(code, redirect_body, location)
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "redirect.json"
                event_id = f"evt-redirect-{code}"
                payload = {"eventId": event_id, "content": "queued note"}
                enqueue_in_process(path, payload)
                before = json.loads(path.read_text(encoding="utf-8"))
                stored_payload = before["events"][0]["payload"]
                self.assertEqual(before["events"][0]["attempts"], 0)
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(path),
                        "--url",
                        origin_base,
                        "--token",
                        token,
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
                origin_hits = origin.hits  # type: ignore[attr-defined]
                target_hits = target.hits  # type: ignore[attr-defined]
                self.assertEqual(len(origin_hits), 1, code)
                self.assertEqual(origin_hits[0]["method"], "POST")
                self.assertEqual(origin_hits[0]["path"], "/cairn-memory/sync/push")
                self.assertTrue(
                    origin_hits[0]["authorization"] == "Bearer " + token,
                    "origin bearer missing",
                )
                self.assertEqual(json.loads(origin_hits[0]["body"])["eventId"], event_id)
                self.assertEqual(len(target_hits), 0, code)
                self.assertNotEqual(completed.returncode, 0)
                self.assertNotIn("Traceback", completed.stderr)
                report = json.loads(completed.stdout)
                self.assertEqual(report["accepted"], 0)
                self.assertEqual(report["failed"], 1)
                self.assertEqual(report["purged"], 0)
                self.assertEqual(report["pending"], 1)
                after = json.loads(path.read_text(encoding="utf-8"))
                self.assertEqual(after["version"], 1)
                self.assertEqual(len(after["events"]), 1)
                self.assertEqual(after["events"][0]["payload"], stored_payload)
                self.assertEqual(after["events"][0]["attempts"], 1)
                visible = "\n".join(
                    (
                        completed.stdout,
                        completed.stderr,
                        path.read_text(encoding="utf-8"),
                    )
                )
                for needle in (token, location_marker, body_marker, target_base, location):
                    if needle in visible:
                        self.fail("replay output or queue included redirect material")
        finally:
            origin.shutdown()
            origin.server_close()
            target.shutdown()
            target.server_close()

    def test_unsupported_scheme_omits_the_location_from_the_stored_error(self) -> None:
        token = "synthetic-device-token"
        location_marker = "synthetic-location-secret"
        body_marker = "synthetic-redirect-body"
        for code in (301, 302, 303, 307, 308):
            with self.subTest(code=code):
                self._assert_scheme_is_not_stored(code, token, location_marker, body_marker)

    def test_non_json_acknowledgement_keeps_the_event_pending(self) -> None:
        server, base = serve_reply(200, b"not json")
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "queue.json"
                enqueue_in_process(path, {"eventId": "evt-nonjson", "content": "kept"})
                done = subprocess.run(
                    [sys.executable, str(OUTBOX_PATH), "replay", str(path), "--url", base, "--token", "t"],
                    text=True,
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(done.returncode, 1, done.stderr)
                self.assertEqual(json.loads(done.stdout)["pending"], 1)
                events = json.loads(path.read_text(encoding="utf-8"))["events"]
                self.assertEqual(events[0]["attempts"], 1)
        finally:
            server.shutdown()
            server.server_close()

    def test_replay_of_a_missing_queue_creates_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "missing.json"
            done = subprocess.run(
                [sys.executable, str(OUTBOX_PATH), "replay", str(path), "--url", "http://127.0.0.1:9", "--token", "t"],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertEqual(json.loads(done.stdout)["note"], "nothing to replay")
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_purge_of_a_missing_queue_creates_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "missing.json"
            done = subprocess.run(
                [sys.executable, str(OUTBOX_PATH), "purge", str(path)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertEqual(json.loads(done.stdout), {"purged": 0, "pending": 0})
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_redirect_close_does_not_read_or_stall(self) -> None:
        self._assert_short_body_stays_pending()
        self._assert_held_body_returns_before_timeout()
        self._assert_missing_location_uses_fixed_error()

    def _assert_scheme_is_not_stored(
        self, code: int, token: str, location_marker: str, body_marker: str
    ) -> None:
        body = json.dumps({"content": body_marker, "token": token}).encode()
        location = f"abc://taken?token={token}&m={location_marker}"
        target, _target_base = serve_reply(200, b'{"accepted": true}')
        origin = _serve_probe(code, body, location, "full")
        try:
            completed, path, stored_payload = self._replay_once(
                origin, f"evt-scheme-{code}", token
            )
            self._assert_pending_once(completed, path, stored_payload, code)
            self.assertEqual(len(origin.hits), 1)
            self.assertEqual(len(target.hits), 0)  # type: ignore[attr-defined]
            self._assert_hidden(
                completed,
                path,
                token,
                location_marker,
                body_marker,
                location,
                "abc://",
            )
        finally:
            _stop(origin)
            _stop(target)

    def _assert_short_body_stays_pending(self) -> None:
        token = "synthetic-device-token"
        location_marker = "synthetic-location-secret"
        body_marker = "synthetic-redirect-body"
        body = json.dumps({"content": body_marker, "token": token}).encode()
        target, target_base = serve_reply(200, b'{"accepted": true}')
        location = f"{target_base}/taken?token={token}&m={location_marker}"
        origin = _serve_probe(302, body, location, "short")
        try:
            started = time.monotonic()
            completed, path, stored_payload = self._replay_once(origin, "evt-short-body", token)
            self.assertLess(time.monotonic() - started, 2)
            self._assert_pending_once(completed, path, stored_payload, 302)
            self.assertEqual(len(origin.hits), 1)
            self.assertEqual(len(target.hits), 0)  # type: ignore[attr-defined]
            if "IncompleteRead" in completed.stderr or "Traceback" in completed.stderr:
                self.fail("redirect read the body")
            self._assert_hidden(
                completed, path, token, location_marker, body_marker, location, target_base
            )
        finally:
            _stop(origin)
            _stop(target)

    def _assert_held_body_returns_before_timeout(self) -> None:
        token = "synthetic-device-token"
        location_marker = "synthetic-location-secret"
        target, target_base = serve_reply(200, b'{"accepted": true}')
        location = f"{target_base}/taken?token={token}&m={location_marker}"
        origin = _serve_probe(302, b"", location, "hold")
        host, port = origin.server_address[:2]
        request = urllib.request.Request(
            f"http://{host}:{port}/cairn-memory/sync/push",
            data=b'{"eventId":"evt-held"}',
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer " + token,
            },
            method="POST",
        )
        holder: dict[str, object] = {}

        def call() -> None:
            try:
                with OUTBOX._OPENER.open(request, timeout=1) as response:
                    holder["opened"] = response.status
            except Exception as error:
                holder["error"] = error

        try:
            started = time.monotonic()
            thread = threading.Thread(target=call)
            thread.start()
            thread.join(2)
            elapsed = time.monotonic() - started
            self.assertFalse(thread.is_alive(), "redirect close blocked")
            self.assertLess(elapsed, 0.5)
            error = holder.get("error")
            if not isinstance(error, urllib.error.HTTPError):
                self.fail("redirect did not close promptly")
            if str(error) != "HTTP Error 302: redirect blocked":
                self.fail("stored error was not the fixed redirect message")
            self.assertEqual(len(origin.hits), 1)
            self.assertEqual(len(target.hits), 0)  # type: ignore[attr-defined]
        finally:
            origin.release.set()  # type: ignore[attr-defined]
            _stop(origin)
            _stop(target)

    def _assert_missing_location_uses_fixed_error(self) -> None:
        token = "synthetic-device-token"
        location_marker = "synthetic-location-secret"
        body_marker = "synthetic-redirect-body"
        body = json.dumps({"content": body_marker, "token": token}).encode()
        origin = _serve_probe(
            302,
            body,
            None,
            "full",
            reason=f"Found {token} {location_marker}",
        )
        try:
            completed, path, stored_payload = self._replay_once(origin, "evt-no-location", token)
            self._assert_pending_once(completed, path, stored_payload, 302)
            self.assertEqual(len(origin.hits), 1)
            self._assert_hidden(completed, path, token, location_marker, body_marker)
        finally:
            _stop(origin)

    def _replay_once(
        self, origin: ThreadingHTTPServer, event_id: str, token: str
    ) -> tuple[subprocess.CompletedProcess[str], Path, dict]:
        host, port = origin.server_address[:2]
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "redirect.json"
        enqueue_in_process(path, {"eventId": event_id, "content": "queued note"})
        stored_payload = json.loads(path.read_text(encoding="utf-8"))["events"][0]["payload"]
        completed = subprocess.run(
            [
                sys.executable,
                str(OUTBOX_PATH),
                "replay",
                str(path),
                "--url",
                f"http://{host}:{port}",
                "--token",
                token,
            ],
            text=True,
            capture_output=True,
            check=False,
            timeout=5,
        )
        return completed, path, stored_payload

    def _assert_pending_once(
        self,
        completed: subprocess.CompletedProcess[str],
        path: Path,
        stored_payload: dict,
        code: int,
    ) -> None:
        self.assertNotEqual(completed.returncode, 0)
        if not completed.stdout.startswith("{"):
            self.fail("replay did not report a pending redirect")
        report = json.loads(completed.stdout)
        self.assertEqual(report["accepted"], 0)
        self.assertEqual(report["failed"], 1)
        self.assertEqual(report["purged"], 0)
        self.assertEqual(report["pending"], 1)
        after = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(after["version"], 1)
        self.assertEqual(after["events"][0]["payload"], stored_payload)
        self.assertEqual(after["events"][0]["attempts"], 1)
        if after["events"][0]["lastError"] != f"HTTP Error {code}: redirect blocked":
            self.fail("stored error was not the fixed redirect message")

    def _assert_hidden(self, completed: subprocess.CompletedProcess[str], path: Path, *needles: str) -> None:
        visible = "\n".join((completed.stdout, completed.stderr, path.read_text(encoding="utf-8")))
        for needle in needles:
            if needle and needle in visible:
                self.fail("replay output or queue included redirect material")


class _ProbeHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        server = self.server
        server.hits.append(self.path.split("?", 1)[0])  # type: ignore[attr-defined]
        self.send_response(server.reply_code, server.reason)  # type: ignore[attr-defined]
        location = server.location  # type: ignore[attr-defined]
        if location:
            self.send_header("Location", location)
        mode = server.mode  # type: ignore[attr-defined]
        body = server.reply_body  # type: ignore[attr-defined]
        if mode == "short":
            self.send_header("Content-Length", "20")
            self.end_headers()
            self.wfile.write(b"tiny")
            self.close_connection = True
            return
        if mode == "hold":
            self.send_header("Content-Length", "1000000")
            self.end_headers()
            server.started.set()  # type: ignore[attr-defined]
            server.release.wait(5)  # type: ignore[attr-defined]
            return
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args) -> None:
        return


def _serve_probe(
    code: int,
    body: bytes,
    location: str | None,
    mode: str,
    reason: str = "Found",
) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _ProbeHandler)
    server.hits = []  # type: ignore[attr-defined]
    server.reply_code = code  # type: ignore[attr-defined]
    server.reply_body = body  # type: ignore[attr-defined]
    server.location = location  # type: ignore[attr-defined]
    server.mode = mode  # type: ignore[attr-defined]
    server.reason = reason  # type: ignore[attr-defined]
    server.started = threading.Event()  # type: ignore[attr-defined]
    server.release = threading.Event()  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


def _stop(server: ThreadingHTTPServer) -> None:
    release = getattr(server, "release", None)
    if release is not None:
        release.set()
    server.shutdown()
    server.server_close()


_BARRIER_CHILD = (
    "import os, sys\n"
    "os.read(int(sys.argv[1]), 1)\n"
    "os.close(int(sys.argv[1]))\n"
    "os.execv(sys.executable, [sys.executable, *sys.argv[2:]])\n"
)


def _overlapped(
    commands: list[tuple[list[str], str | None]]
) -> list[tuple[int, str, str]]:
    """Start each real outbox command, then release them on one pipe."""
    read_fd, write_fd = os.pipe()
    procs: list[subprocess.Popen[str]] = []
    try:
        for args, stdin_text in commands:
            proc = subprocess.Popen(
                [sys.executable, "-c", _BARRIER_CHILD, str(read_fd), str(OUTBOX_PATH), *args],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                pass_fds=(read_fd,),
            )
            if stdin_text is not None and proc.stdin is not None:
                proc.stdin.write(stdin_text)
            if proc.stdin is not None:
                proc.stdin.close()
            procs.append(proc)
        os.close(read_fd)
        read_fd = -1
        os.write(write_fd, b"x" * len(procs))
        os.close(write_fd)
        write_fd = -1
        results = []
        for proc in procs:
            return_code = proc.wait(timeout=10)
            stdout = proc.stdout.read() if proc.stdout is not None else ""
            stderr = proc.stderr.read() if proc.stderr is not None else ""
            results.append((return_code, stdout, stderr))
        return results
    finally:
        if read_fd >= 0:
            os.close(read_fd)
        if write_fd >= 0:
            os.close(write_fd)
        for proc in procs:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=5)
            if proc.stdout is not None:
                proc.stdout.close()
            if proc.stderr is not None:
                proc.stderr.close()


class _HoldHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(length)
        server = self.server
        server.arrived.set()  # type: ignore[attr-defined]
        server.release.wait(10)  # type: ignore[attr-defined]
        self.send_response(500)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, format: str, *args) -> None:
        return


def _serve_hold() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _HoldHandler)
    server.daemon_threads = True
    server.arrived = threading.Event()  # type: ignore[attr-defined]
    server.release = threading.Event()  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address[:2]
    return server, f"http://{host}:{port}"


class OutboxLockCheck(unittest.TestCase):
    def test_concurrent_processes_keep_distinct_ids(self) -> None:
        for _round in range(2):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "distinct.json"
                count = 8
                commands = [
                    (
                        ["enqueue", str(path)],
                        json.dumps({"eventId": f"evt-distinct-{index}", "content": "note"}),
                    )
                    for index in range(count)
                ]
                results = _overlapped(commands)
                for return_code, _stdout, stderr in results:
                    self.assertEqual(return_code, 0, stderr)
                stored = json.loads(path.read_text(encoding="utf-8"))
                ids = [item["payload"]["eventId"] for item in stored["events"]]
                self.assertEqual(
                    sorted(ids),
                    [f"evt-distinct-{index}" for index in range(count)],
                )
                self.assertEqual(len(ids), count)
                self.assertEqual(stored["version"], 1)

    def test_concurrent_duplicate_id_is_stored_once(self) -> None:
        for _round in range(8):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "duplicate.json"
                commands = [
                    (
                        ["enqueue", str(path)],
                        json.dumps({"eventId": "evt-same", "content": "same-note"}),
                    )
                    for _index in range(8)
                ]
                results = _overlapped(commands)
                for return_code, _stdout, stderr in results:
                    self.assertEqual(return_code, 0, stderr)
                queued = sum(1 for _code, stdout, _stderr in results if '"queued": true' in stdout)
                stored = json.loads(path.read_text(encoding="utf-8"))["events"]
                self.assertEqual(len(stored), 1)
                self.assertEqual(stored[0]["payload"]["eventId"], "evt-same")
                self.assertEqual(queued, 1)

    def test_enqueue_waits_for_held_replay_and_keeps_both_events(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "held.json"
            path.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "events": [
                            {
                                "payload": {"eventId": "evt-existing", "content": "existing-note"},
                                "attempts": 0,
                                "lastError": None,
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            server, base = _serve_hold()
            replay = subprocess.Popen(
                [
                    sys.executable,
                    str(OUTBOX_PATH),
                    "replay",
                    str(path),
                    "--url",
                    base,
                    "--token",
                    "synthetic-device-token",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            enqueue = None
            try:
                self.assertTrue(server.arrived.wait(5), "replay did not reach the stub")
                enqueue = subprocess.Popen(
                    [sys.executable, str(OUTBOX_PATH), "enqueue", str(path)],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                assert enqueue.stdin is not None
                enqueue.stdin.write(
                    json.dumps({"eventId": "evt-added", "content": "added-note"})
                )
                enqueue.stdin.close()
                try:
                    enqueue.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    blocked = True
                else:
                    blocked = False
                self.assertTrue(blocked, "enqueue did not wait for the held replay")
                self.assertIsNone(enqueue.poll())
                server.release.set()
                replay_code = replay.wait(timeout=10)
                replay_out = replay.stdout.read() if replay.stdout is not None else ""
                replay_err = replay.stderr.read() if replay.stderr is not None else ""
                self.assertEqual(replay_code, 1, replay_err)
                self.assertEqual(json.loads(replay_out)["accepted"], 0)
                enqueue_code = enqueue.wait(timeout=10)
                enqueue_out = enqueue.stdout.read() if enqueue.stdout is not None else ""
                enqueue_err = enqueue.stderr.read() if enqueue.stderr is not None else ""
                self.assertEqual(enqueue_code, 0, enqueue_err)
                self.assertIn('"queued": true', enqueue_out)
                stored = json.loads(path.read_text(encoding="utf-8"))
                self.assertEqual(
                    [item["payload"]["eventId"] for item in stored["events"]],
                    ["evt-existing", "evt-added"],
                )
                self.assertEqual(stored["events"][0]["payload"]["content"], "existing-note")
                self.assertEqual(stored["events"][0]["attempts"], 1)
                self.assertIn("500", stored["events"][0]["lastError"])
                self.assertEqual(stored["events"][1]["attempts"], 0)
                self.assertNotIn(b"synthetic-device-token", path.read_bytes())
            finally:
                server.release.set()
                _stop(server)
                for proc in (replay, enqueue):
                    if proc is None:
                        continue
                    if proc.poll() is None:
                        proc.kill()
                        proc.wait(timeout=5)
                    for stream in (proc.stdin, proc.stdout, proc.stderr):
                        if stream is not None and not stream.closed:
                            stream.close()

    def test_purge_and_enqueue_keep_the_accepted_event(self) -> None:
        for _round in range(2):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "purge.json"
                path.write_text(
                    json.dumps(
                        {
                            "version": 1,
                            "events": [
                                {
                                    "payload": {"eventId": "evt-old", "content": "keep"},
                                    "attempts": 0,
                                    "lastError": None,
                                }
                            ],
                        }
                    ),
                    encoding="utf-8",
                )
                results = _overlapped(
                    [
                        (
                            ["enqueue", str(path)],
                            json.dumps({"eventId": "evt-new", "content": "added"}),
                        ),
                        (["purge", str(path), "--event-id", "evt-old"], None),
                    ]
                )
                for return_code, _stdout, stderr in results:
                    self.assertEqual(return_code, 0, stderr)
                stored = json.loads(path.read_text(encoding="utf-8"))
                self.assertEqual(
                    [item["payload"]["eventId"] for item in stored["events"]],
                    ["evt-new"],
                )
                self.assertEqual(stored["events"][0]["payload"]["content"], "added")
                self.assertEqual(stored["version"], 1)

    def test_failed_replace_keeps_the_queue_and_drops_the_temp(self) -> None:
        marker = "synthetic-save-body"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "save.json"
            path.write_text(
                json.dumps(
                    {
                        "events": [
                            {
                                "attempts": 0,
                                "lastError": None,
                                "payload": {"content": "kept-note", "eventId": "evt-kept"},
                            }
                        ],
                        "version": 1,
                    }
                ),
                encoding="utf-8",
            )
            os.chmod(path, 0o640)
            before = path.read_bytes()
            original = OUTBOX.os.replace

            def fail_replace(src: str, dst: str) -> None:
                raise OSError("replace failed")

            OUTBOX.os.replace = fail_replace
            try:
                with self.assertRaises(OSError):
                    enqueue_in_process(
                        path,
                        {"eventId": "evt-save-fail", "content": marker},
                    )
            finally:
                OUTBOX.os.replace = original
            self.assertEqual(path.read_bytes(), before)
            self.assertEqual(path.stat().st_mode & 0o777, 0o640)
            for item in Path(directory).iterdir():
                if item == path or item.name.endswith(".lock"):
                    continue
                payload = item.read_bytes()
                self.assertNotIn(marker.encode(), payload)
                self.fail(f"temp file left behind: {item.name}")
            lock_path = Path(str(path) + ".lock")
            if lock_path.exists():
                self.assertNotIn(marker.encode(), lock_path.read_bytes())
            completed = subprocess.run(
                [sys.executable, str(OUTBOX_PATH), "enqueue", str(path)],
                input=json.dumps({"eventId": "evt-after", "content": "after-note"}),
                text=True,
                capture_output=True,
                check=False,
                timeout=5,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertNotIn(marker, completed.stdout + completed.stderr)
            stored = json.loads(path.read_text(encoding="utf-8"))
            ids = [item["payload"]["eventId"] for item in stored["events"]]
            self.assertEqual(ids, ["evt-kept", "evt-after"])
            self.assertNotIn(marker, path.read_text(encoding="utf-8"))
            self.assertEqual(path.stat().st_mode & 0o777, 0o640)

    def test_existing_mode_is_kept_and_the_lock_file_is_not_the_queue(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mode.json"
            enqueue_in_process(path, {"eventId": "evt-mode", "content": "note"})
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            os.chmod(path, 0o640)
            lock_path = Path(str(path) + ".lock")
            self.assertTrue(lock_path.exists())
            inode = lock_path.stat().st_ino
            enqueue_in_process(path, {"eventId": "evt-mode-2", "content": "note"})
            self.assertEqual(path.stat().st_mode & 0o777, 0o640)
            self.assertEqual(lock_path.stat().st_ino, inode)
            self.assertEqual(lock_path.stat().st_mode & 0o777, 0o600)
            missing = Path(directory) / "missing.json"
            failed = enqueue_cli(missing, {"content": "synthetic-save-body"})
            self.assertNotEqual(failed.returncode, 0)
            self.assertNotIn('"queued": true', failed.stdout)
            self.assertNotIn("synthetic-save-body", failed.stdout + failed.stderr)
            self.assertFalse(missing.exists())
            rejected_lock = Path(str(missing) + ".lock")
            if rejected_lock.exists():
                self.assertEqual(rejected_lock.read_bytes(), b"")

    def test_lock_failure_does_not_write_the_queue(self) -> None:
        marker = "synthetic-save-body"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "closed.json"
            path.write_text(
                json.dumps({"version": 1, "events": []}),
                encoding="utf-8",
            )
            before = path.read_bytes()
            original = OUTBOX.fcntl
            OUTBOX.fcntl = None
            try:
                with self.assertRaises(RuntimeError) as missing:
                    enqueue_in_process(path, {"eventId": "evt-nolock", "content": marker})
            finally:
                OUTBOX.fcntl = original
            self.assertEqual(str(missing.exception), OUTBOX._LOCK_MESSAGE)
            self.assertNotIn("Windows", str(missing.exception))
            self.assertNotIn(marker, str(missing.exception))
            self.assertEqual(path.read_bytes(), before)

            def fail_flock(fd: int, operation: int) -> None:
                raise OSError("flock failed")

            real_flock = OUTBOX.fcntl.flock
            OUTBOX.fcntl.flock = fail_flock
            try:
                with self.assertRaises(RuntimeError) as failed:
                    enqueue_in_process(path, {"eventId": "evt-flock", "content": marker})
            finally:
                OUTBOX.fcntl.flock = real_flock
            self.assertEqual(str(failed.exception), OUTBOX._LOCK_MESSAGE)
            self.assertNotIn(marker, str(failed.exception))
            self.assertEqual(path.read_bytes(), before)

    def test_same_thread_reentry_locks_once(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "reentry.json"
            calls = {"n": 0}
            real = OUTBOX.fcntl.flock
            lock_ex = OUTBOX.fcntl.LOCK_EX

            def counting(fd: int, operation: int) -> None:
                if operation == lock_ex:
                    calls["n"] += 1
                real(fd, operation)

            OUTBOX.fcntl.flock = counting
            try:
                done = threading.Event()
                holder: dict[str, object] = {}

                def run() -> None:
                    try:
                        with OUTBOX._queue_lock(path):
                            enqueue_in_process(
                                path,
                                {"eventId": "evt-reentry", "content": "note"},
                            )
                        holder["ok"] = True
                    except Exception as error:
                        holder["error"] = error
                    finally:
                        done.set()

                thread = threading.Thread(target=run, daemon=True)
                thread.start()
                self.assertTrue(done.wait(3), "reentry deadlocked")
            finally:
                OUTBOX.fcntl.flock = real
            if "error" in holder:
                raise holder["error"]  # type: ignore[misc]
            self.assertEqual(calls["n"], 1)
            stored = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(stored["events"][0]["payload"]["eventId"], "evt-reentry")

    def test_alias_reentry_keeps_the_other_process_locked(self) -> None:
        root, link, path_a, path_b = _alias_queue_paths()
        inner = Path(root) / "inner"
        after = Path(root) / "after"
        leave_inner = Path(root) / "leave-inner"
        leave_outer = Path(root) / "leave-outer"
        child = subprocess.Popen(
            [
                sys.executable,
                "-c",
                _ALIAS_CHILD,
                str(OUTBOX_PATH),
                str(path_a),
                str(path_b),
                str(inner),
                str(after),
                str(leave_inner),
                str(leave_outer),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            self.assertNotEqual(os.path.abspath(path_a), os.path.abspath(path_b))
            if not _wait_for(inner, 3):
                child.kill()
                stdout, stderr = child.communicate(timeout=3)
                self.fail(f"alias reentry did not return within 3s: {stderr or stdout}")
            lock_a = Path(os.path.abspath(path_a) + ".lock")
            lock_b = Path(os.path.abspath(path_b) + ".lock")
            self.assertEqual(
                (lock_a.stat().st_dev, lock_a.stat().st_ino),
                (lock_b.stat().st_dev, lock_b.stat().st_ino),
            )
            self.assertEqual(_probe_lock(lock_a), "busy")
            leave_inner.write_text("x", encoding="utf-8")
            self.assertTrue(_wait_for(after, 3), "outer lock was dropped with the inner call")
            self.assertEqual(_probe_lock(lock_a), "busy")
            leave_outer.write_text("x", encoding="utf-8")
            return_code = child.wait(timeout=3)
            stderr = child.stderr.read() if child.stderr is not None else ""
            self.assertEqual(return_code, 0, stderr)
            self.assertEqual(_probe_lock(lock_a), "acquired")
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=3)
            for stream in (child.stdout, child.stderr):
                if stream is not None and not stream.closed:
                    stream.close()
            if link is not None:
                os.unlink(link)
            shutil.rmtree(root)


def _wait_for(path: Path, seconds: float) -> bool:
    deadline = time.time() + seconds
    while time.time() < deadline:
        if path.exists():
            return True
        time.sleep(0.05)
    return False


def _alias_queue_paths() -> tuple[str, str | None, Path, Path]:
    temporary = Path("/tmp")
    private = Path("/private/tmp")
    if temporary.is_symlink() and private.is_dir() and temporary.resolve() == private.resolve():
        root = tempfile.mkdtemp(prefix="cairn-outbox-alias-", dir="/tmp")
        name = Path(root).name
        return root, None, temporary / name / "queue.json", private / name / "queue.json"
    root = tempfile.mkdtemp(prefix="cairn-outbox-alias-")
    link = root + "-alias"
    os.symlink(root, link)
    return root, link, Path(root) / "queue.json", Path(link) / "queue.json"


_ALIAS_CHILD = """
import importlib.util
import os
import sys
import time

spec = importlib.util.spec_from_file_location("cairn_outbox", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
path_a, path_b, inner, after, leave_inner, leave_outer = sys.argv[2:8]

def wait(path):
    deadline = time.time() + 4
    while not os.path.exists(path):
        if time.time() > deadline:
            raise SystemExit("timed out")
        time.sleep(0.05)

with module._queue_lock(module.Path(path_a)):
    with module._queue_lock(module.Path(path_b)):
        open(inner, "w").close()
        wait(leave_inner)
    open(after, "w").close()
    wait(leave_outer)
"""


def _probe_lock(lock_path: Path) -> str:
    completed = subprocess.run(
        [sys.executable, "-c", _LOCK_PROBE, str(lock_path)],
        text=True,
        capture_output=True,
        check=False,
        timeout=3,
    )
    if completed.returncode != 0:
        raise AssertionError(completed.stderr)
    return completed.stdout.strip()


_LOCK_PROBE = """
import errno
import fcntl
import os
import sys

fd = os.open(sys.argv[1], os.O_RDWR | os.O_CLOEXEC)
try:
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        if error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
            print("busy")
        else:
            raise
    else:
        fcntl.flock(fd, fcntl.LOCK_UN)
        print("acquired")
finally:
    os.close(fd)
"""


_BOUND_TOKEN = "synthetic-device-token-v2"
_ACCOUNT_TOKEN = "synthetic-account-token-v2"
_OTHER_DEVICE = "synthetic-other-device-token"


def _bound_text(path: Path, event: dict, origin: str, token: str = _BOUND_TOKEN) -> str:
    buffer = io.StringIO()
    with redirect_stdout(buffer):
        code = OUTBOX.enqueue_bound(path, event, origin, token)
    if code != 0:
        raise AssertionError(f"enqueue_bound returned {code}")
    return buffer.getvalue()


def _replay_text(path: Path, origin: str, token: str = _BOUND_TOKEN) -> tuple[int, str]:
    buffer = io.StringIO()
    with redirect_stdout(buffer):
        code = OUTBOX.replay(path, origin, token)
    return code, buffer.getvalue()


class OutboxBoundCheck(unittest.TestCase):
    def setUp(self) -> None:
        self._account = os.environ.get("CAIRN_ACCOUNT_TOKEN")
        self._device = os.environ.get("CAIRN_DEVICE_TOKEN")
        os.environ.pop("CAIRN_ACCOUNT_TOKEN", None)
        os.environ.pop("CAIRN_DEVICE_TOKEN", None)

    def tearDown(self) -> None:
        self._restore("CAIRN_ACCOUNT_TOKEN", self._account)
        self._restore("CAIRN_DEVICE_TOKEN", self._device)

    def _restore(self, name: str, value: str | None) -> None:
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value

    def _assert_hidden(self, text: str, *needles: str) -> None:
        for needle in needles:
            if needle and needle in text:
                self.fail("output included secret material")

    def test_bound_file_keeps_the_redacted_body_and_not_the_token(self) -> None:
        self.assertEqual(OUTBOX.normalize_origin("http://127.0.0.1:80/"), "http://127.0.0.1")
        self.assertEqual(OUTBOX.normalize_origin("https://Example.COM:443"), "https://example.com")
        self.assertEqual(OUTBOX.normalize_origin("HTTP://127.0.0.1:3111/"), "http://127.0.0.1:3111")
        self.assertEqual(OUTBOX.normalize_origin("http://[::1]:3111"), "http://[::1]:3111")
        self.assertEqual(OUTBOX.normalize_origin("http://[::1]:80"), "http://[::1]")
        self.assertEqual(OUTBOX.normalize_origin("http://localhost:3111/"), "http://localhost:3111")
        self.assertEqual(OUTBOX.normalize_origin("http://localhost:80/"), "http://localhost")
        for raw in (
            "http://user:synthetic-userinfo@127.0.0.1:3111",
            "http://127.0.0.1:3111?token=synthetic-query-secret",
            "http://127.0.0.1:3111#synthetic-fragment",
            "http://127.0.0.1:3111/cairn-memory",
            "http://127.0.0.1:3111/cairn-memory/sync/push",
        ):
            with self.assertRaises(ValueError) as rejected:
                OUTBOX.normalize_origin(raw)
            self.assertEqual(str(rejected.exception), OUTBOX._ORIGIN_MESSAGE)
            self._assert_hidden(
                str(rejected.exception),
                "synthetic-userinfo",
                "synthetic-query-secret",
                "synthetic-fragment",
                _BOUND_TOKEN,
            )

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bound.json"
            prose = "ordinary queued note"
            with self.assertRaises(ValueError) as bad_origin:
                OUTBOX.enqueue_bound(
                    path,
                    {"eventId": "evt-origin", "content": prose},
                    "http://user:synthetic-userinfo@127.0.0.1:9/cairn-memory",
                    _BOUND_TOKEN,
                )
            self.assertEqual(str(bad_origin.exception), OUTBOX._ORIGIN_MESSAGE)
            self.assertFalse(path.exists())
            self._assert_hidden(str(bad_origin.exception), "synthetic-userinfo", _BOUND_TOKEN)
            for item in Path(directory).iterdir():
                self.fail(f"failed create left {item.name}")

            origin = "http://127.0.0.1:3111/"
            queued = _bound_text(
                path,
                {
                    "eventId": "evt-bound",
                    "content": f"{prose} {SK}",
                    "context": {"files": [f"src/{SK}", "src/kept.rs"], "branch": "sk-looking-name"},
                },
                origin,
            )
            self.assertIn('"queued": true', queued)
            raw = path.read_bytes()
            stored = json.loads(raw)
            fingerprint = OUTBOX.device_fingerprint(_BOUND_TOKEN)
            self.assertEqual(stored["version"], 2)
            self.assertEqual(stored["route"], "sync/push")
            self.assertEqual(stored["origin"], "http://127.0.0.1:3111")
            self.assertEqual(stored["deviceFingerprint"], fingerprint)
            self.assertEqual(stored["events"][0]["payload"]["content"], f"{prose} [REDACTED]")
            self.assertEqual(stored["events"][0]["payload"]["context"]["files"][1], "src/kept.rs")
            self.assertEqual(stored["events"][0]["payload"]["context"]["branch"], "sk-looking-name")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            visible = queued + path.read_text(encoding="utf-8")
            self._assert_hidden(visible, _BOUND_TOKEN, SK, "Bearer")
            self.assertNotIn(fingerprint, queued)

            duplicate = _bound_text(
                path,
                {"eventId": "evt-bound", "content": f"replacement {SK}"},
                "HTTP://127.0.0.1:3111",
            )
            self.assertIn('"duplicate": true', duplicate)
            self.assertEqual(len(json.loads(path.read_text(encoding="utf-8"))["events"]), 1)
            third = _bound_text(path, {"eventId": "evt-next", "content": "second note"}, origin)
            self.assertIn('"queued": true', third)
            ids = [
                item["payload"]["eventId"]
                for item in json.loads(path.read_text(encoding="utf-8"))["events"]
            ]
            self.assertEqual(ids, ["evt-bound", "evt-next"])
            self._assert_hidden(path.read_text(encoding="utf-8"), _BOUND_TOKEN, SK)

            os.environ["CAIRN_ACCOUNT_TOKEN"] = _BOUND_TOKEN
            fresh = Path(directory) / "account.json"
            before_files = sorted(item.name for item in Path(directory).iterdir())
            with self.assertRaises(ValueError) as account:
                OUTBOX.enqueue_bound(
                    fresh,
                    {"eventId": "evt-account", "content": prose},
                    origin,
                    _BOUND_TOKEN,
                )
            self.assertEqual(str(account.exception), OUTBOX._ACCOUNT_TOKEN_MESSAGE)
            self.assertFalse(fresh.exists())
            self._assert_hidden(str(account.exception), _BOUND_TOKEN)
            self.assertEqual(sorted(item.name for item in Path(directory).iterdir()), before_files)
            os.environ.pop("CAIRN_ACCOUNT_TOKEN", None)

            original = OUTBOX.os.replace

            def fail_replace(src: str, dst: str) -> None:
                raise OSError("replace failed")

            failed = Path(directory) / "failed.json"
            OUTBOX.os.replace = fail_replace
            try:
                with self.assertRaises(OSError):
                    OUTBOX.enqueue_bound(
                        failed,
                        {"eventId": "evt-fail", "content": f"temp {SK}"},
                        origin,
                        _BOUND_TOKEN,
                    )
            finally:
                OUTBOX.os.replace = original
            self.assertFalse(failed.exists())
            for item in Path(directory).iterdir():
                if ".tmp" in item.name or item.name == "failed.json":
                    self.fail(f"temp file left behind: {item.name}")
                self.assertNotIn(SK.encode(), item.read_bytes())

    def test_plain_http_is_allowed_only_for_loopback(self) -> None:
        for raw in (
            "http://example.com",
            "http://example.com:3111",
            "http://10.0.0.5:3111",
            "http://localhost.example.com",
            "http://127.0.0.1.example.com",
            "http://[::2]:3111",
            "http://[::ffff:127.0.0.1]:3111",
        ):
            with self.subTest(url=raw):
                with self.assertRaises(ValueError) as rejected:
                    OUTBOX.normalize_origin(raw)
                self.assertEqual(str(rejected.exception), OUTBOX._TRANSPORT_MESSAGE)
        self.assertEqual(OUTBOX.normalize_origin("http://127.0.0.2:3111"), "http://127.0.0.2:3111")
        self.assertEqual(OUTBOX.normalize_origin("https://10.0.0.5:3111"), "https://10.0.0.5:3111")

    def test_unbound_url_check_and_version1_replay_refuse_plain_http(self) -> None:
        OUTBOX.require_secure_url("http://127.0.0.1:3111/prefix/")
        OUTBOX.require_secure_url("https://example.com/prefix")
        for raw in (
            "http://example.com",
            "ftp://127.0.0.1",
            "file:///etc/passwd",
            "http://user:pw@127.0.0.1:3111",
            "http://127.0.0.1:3111?token=x",
            "http://127.0.0.1:3111#x",
        ):
            with self.subTest(url=raw):
                with self.assertRaises(ValueError):
                    OUTBOX.require_secure_url(raw)
        calls = {"n": 0}
        originals = (OUTBOX._OPENER.open, OUTBOX._LOOPBACK_OPENER.open)

        def refuse_open(*_args, **_kwargs):
            calls["n"] += 1
            raise AssertionError("opener called")

        OUTBOX._OPENER.open = OUTBOX._LOOPBACK_OPENER.open = refuse_open
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "v1.json"
                enqueue_in_process(path, {"eventId": "evt-plain-http", "content": "x"})
                before = path.read_bytes()
                with self.assertRaises(ValueError):
                    OUTBOX.replay(path, "http://example.com", "synthetic-device-token")
                self.assertEqual(path.read_bytes(), before)
        finally:
            OUTBOX._OPENER.open, OUTBOX._LOOPBACK_OPENER.open = originals
        self.assertEqual(calls["n"], 0)

    def test_percent_host_is_refused_before_enqueue_or_replay(self) -> None:
        encoded = (
            "http://127.0.0.1%2eattacker.example:3111",
            "http://127.0.0.1%2Eattacker.example:3111",
            "http://evil.example%3a8443",
            "http://127.0.0.1%00.example",
            "http://[fe80::1%25lo0]:3111",
            "http://[fe80::1%lo0]",
        )
        calls = {"n": 0}
        original = OUTBOX._OPENER.open

        def refuse_open(*_args, **_kwargs):
            calls["n"] += 1
            raise AssertionError("opener called")

        OUTBOX._OPENER.open = refuse_open
        try:
            with tempfile.TemporaryDirectory() as directory:
                missing = Path(directory) / "missing.json"
                existing = Path(directory) / "existing.json"
                _bound_text(
                    existing,
                    {"eventId": "evt-keep", "content": "kept"},
                    "http://127.0.0.1:3111/",
                )
                before = existing.read_bytes()
                for raw in encoded:
                    with self.assertRaises(ValueError) as created:
                        OUTBOX.enqueue_bound(
                            missing,
                            {"eventId": "evt-bad", "content": "note"},
                            raw,
                            _BOUND_TOKEN,
                        )
                    self.assertEqual(str(created.exception), OUTBOX._ORIGIN_MESSAGE)
                    self._assert_hidden(str(created.exception), raw, _BOUND_TOKEN, "%")
                    self.assertFalse(missing.exists())
                    with self.assertRaises(ValueError) as queued:
                        OUTBOX.enqueue_bound(
                            existing,
                            {"eventId": "evt-add", "content": "note"},
                            raw,
                            _BOUND_TOKEN,
                        )
                    self.assertEqual(str(queued.exception), OUTBOX._ORIGIN_MESSAGE)
                    self.assertEqual(existing.read_bytes(), before)
                    with self.assertRaises(ValueError) as replayed:
                        OUTBOX.replay(existing, raw, _BOUND_TOKEN)
                    self.assertEqual(str(replayed.exception), OUTBOX._ORIGIN_MESSAGE)
                    self._assert_hidden(str(replayed.exception), raw, _BOUND_TOKEN, "%")
                    self.assertEqual(existing.read_bytes(), before)
                self.assertEqual(calls["n"], 0)
                self.assertFalse(any(item.name.startswith("missing") for item in Path(directory).iterdir()))
        finally:
            OUTBOX._OPENER.open = original

    def test_mismatch_makes_no_request_and_leaves_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "check.json"
            server, base = serve([])
            try:
                _bound_text(
                    path,
                    {"eventId": "evt-stay", "content": f"kept {SK}"},
                    base + "/",
                )
                before = path.read_bytes()
                fingerprint = json.loads(before)["deviceFingerprint"]
                os.environ["CAIRN_DEVICE_TOKEN"] = _OTHER_DEVICE
                wrong_origin = "http://127.0.0.1:9"
                checks = (
                    (wrong_origin, _BOUND_TOKEN, "outbox origin does not match this queue", None),
                    (base, _OTHER_DEVICE, "outbox device token does not match this queue", None),
                    (base, _BOUND_TOKEN, OUTBOX._ACCOUNT_TOKEN_MESSAGE, _BOUND_TOKEN),
                    (
                        "http://user:synthetic-userinfo@" + base.split("://", 1)[1],
                        _BOUND_TOKEN,
                        OUTBOX._ORIGIN_MESSAGE,
                        "synthetic-userinfo",
                    ),
                )
                os.environ["CAIRN_ACCOUNT_TOKEN"] = _ACCOUNT_TOKEN
                for url, token, message, secret in checks:
                    if message == OUTBOX._ACCOUNT_TOKEN_MESSAGE:
                        os.environ["CAIRN_ACCOUNT_TOKEN"] = token
                    else:
                        os.environ["CAIRN_ACCOUNT_TOKEN"] = _ACCOUNT_TOKEN
                    with self.assertRaises(ValueError) as failed:
                        OUTBOX.replay(path, url, token)
                    self.assertEqual(str(failed.exception), message)
                    self.assertEqual(path.read_bytes(), before)
                    hidden = [token, _BOUND_TOKEN, SK, fingerprint, "Bearer"]
                    if secret:
                        hidden.append(secret)
                    self._assert_hidden(str(failed.exception), *hidden)
                self.assertEqual(server.bodies, [])  # type: ignore[attr-defined]

                route = json.loads(before)
                route["route"] = "observe"
                path.write_text(json.dumps(route, indent=2, sort_keys=True) + "\n", encoding="utf-8")
                routed = path.read_bytes()
                with self.assertRaises(ValueError) as wrong_route:
                    OUTBOX.replay(path, base, _BOUND_TOKEN)
                self.assertEqual(str(wrong_route.exception), "outbox route does not match this queue")
                self.assertEqual(path.read_bytes(), routed)
                self.assertEqual(server.bodies, [])  # type: ignore[attr-defined]

                unknown = Path(directory) / "unknown.json"
                unknown.write_bytes(b'{"events": [], "version": 3}\n')
                unknown_bytes = unknown.read_bytes()
                with self.assertRaises(ValueError) as bad_version:
                    OUTBOX.replay(unknown, base, _BOUND_TOKEN)
                self.assertEqual(str(bad_version.exception), OUTBOX._VERSION_MESSAGE)
                self.assertEqual(unknown.read_bytes(), unknown_bytes)

                invalid = Path(directory) / "invalid.json"
                invalid.write_bytes(b'{"events": [], "route": "sync/push", "version": 2}\n')
                invalid_bytes = invalid.read_bytes()
                with self.assertRaises(ValueError) as bad_meta:
                    OUTBOX.replay(invalid, base, _BOUND_TOKEN)
                self.assertEqual(str(bad_meta.exception), OUTBOX._METADATA_MESSAGE)
                self.assertEqual(invalid.read_bytes(), invalid_bytes)
                with self.assertRaises(ValueError) as purge_unknown:
                    OUTBOX.purge(unknown, None)
                self.assertEqual(str(purge_unknown.exception), OUTBOX._VERSION_MESSAGE)
                self.assertEqual(unknown.read_bytes(), unknown_bytes)
                with self.assertRaises(ValueError) as purge_invalid:
                    OUTBOX.purge(invalid, None)
                self.assertEqual(str(purge_invalid.exception), OUTBOX._METADATA_MESSAGE)
                self.assertEqual(invalid.read_bytes(), invalid_bytes)
                self.assertEqual(server.bodies, [])  # type: ignore[attr-defined]
            finally:
                server.shutdown()
                server.server_close()

    def test_matching_replay_removes_accepted_events_and_purges_401(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "replay.json"
            server, base = serve([200, 500])
            try:
                _bound_text(path, {"eventId": "evt-1", "content": "alpha"}, base)
                _bound_text(path, {"eventId": "evt-2", "content": f"beta {SK}"}, base + "/")
                os.environ["CAIRN_DEVICE_TOKEN"] = _OTHER_DEVICE
                os.environ["CAIRN_ACCOUNT_TOKEN"] = _ACCOUNT_TOKEN
                code, report = _replay_text(path, base, _BOUND_TOKEN)
                self.assertEqual(code, 1, report)
                stored = json.loads(path.read_text(encoding="utf-8"))
                self.assertEqual([item["payload"]["eventId"] for item in stored["events"]], ["evt-2"])
                self.assertEqual(stored["events"][0]["attempts"], 1)
                self.assertNotIn(SK, stored["events"][0]["payload"]["content"])
                self.assertEqual(stored["version"], 2)
                self.assertEqual(stored["origin"], base)
                posted = [json.loads(body) for body in server.bodies]  # type: ignore[attr-defined]
                self.assertEqual([item["eventId"] for item in posted], ["evt-1", "evt-2"])
                self._assert_hidden(report + path.read_text(encoding="utf-8"), _BOUND_TOKEN, SK)
                self.assertNotIn(stored["deviceFingerprint"], report)
            finally:
                server.shutdown()
                server.server_close()

            accepted_path = Path(directory) / "accepted.json"
            accepted, accepted_base = serve([])
            try:
                _bound_text(accepted_path, {"eventId": "evt-a", "content": "one"}, accepted_base)
                _bound_text(accepted_path, {"eventId": "evt-b", "content": "two"}, accepted_base)
                code, report = _replay_text(accepted_path, accepted_base + "/")
                self.assertEqual(code, 0, report)
                self.assertEqual(json.loads(report)["accepted"], 2)
                stored = json.loads(accepted_path.read_text(encoding="utf-8"))
                self.assertEqual(stored["events"], [])
                self.assertEqual(stored["version"], 2)
                self.assertEqual(stored["deviceFingerprint"], OUTBOX.device_fingerprint(_BOUND_TOKEN))
                posted = [json.loads(body)["eventId"] for body in accepted.bodies]  # type: ignore[attr-defined]
                self.assertEqual(posted, ["evt-a", "evt-b"])
            finally:
                accepted.shutdown()
                accepted.server_close()

            denied_path = Path(directory) / "denied.json"
            denied, denied_base = serve([401])
            try:
                _bound_text(denied_path, {"eventId": "evt-deny-1", "content": "one"}, denied_base)
                _bound_text(denied_path, {"eventId": "evt-deny-2", "content": f"two {SK}"}, denied_base)
                fingerprint = json.loads(denied_path.read_text(encoding="utf-8"))["deviceFingerprint"]
                code, report = _replay_text(denied_path, denied_base)
                self.assertEqual(code, 1)
                parsed = json.loads(report)
                self.assertEqual(parsed["purged"], 2)
                self.assertEqual(parsed["pending"], 0)
                stored = json.loads(denied_path.read_text(encoding="utf-8"))
                self.assertEqual(stored["events"], [])
                self.assertEqual(stored["version"], 2)
                self.assertEqual(stored["route"], "sync/push")
                self.assertEqual(stored["origin"], denied_base)
                self.assertEqual(stored["deviceFingerprint"], fingerprint)
                self._assert_hidden(report, _BOUND_TOKEN, SK, fingerprint)
                self._assert_hidden(denied_path.read_text(encoding="utf-8"), _BOUND_TOKEN, SK)
            finally:
                denied.shutdown()
                denied.server_close()

            redirect_path = Path(directory) / "redirect.json"
            target, target_url = serve_reply(200, b'{"accepted": true, "marker": "synthetic-redirect-body"}')
            origin, origin_url = serve_reply(
                302,
                b'{"marker":"synthetic-redirect-body"}',
                target_url + "/stolen",
            )
            try:
                _bound_text(redirect_path, {"eventId": "evt-redirect", "content": "stay"}, origin_url)
                before_payload = json.loads(redirect_path.read_text(encoding="utf-8"))["events"][0]["payload"]
                code, report = _replay_text(redirect_path, origin_url)
                self.assertEqual(code, 1)
                stored = json.loads(redirect_path.read_text(encoding="utf-8"))
                self.assertEqual(stored["version"], 2)
                self.assertEqual(stored["events"][0]["payload"], before_payload)
                self.assertEqual(stored["events"][0]["attempts"], 1)
                self.assertEqual(stored["events"][0]["lastError"], "HTTP Error 302: redirect blocked")
                self.assertEqual(len(origin.hits), 1)  # type: ignore[attr-defined]
                self.assertEqual(origin.hits[0]["method"], "POST")  # type: ignore[attr-defined]
                self.assertEqual(origin.hits[0]["path"], "/cairn-memory/sync/push")  # type: ignore[attr-defined]
                self.assertEqual(target.hits, [])  # type: ignore[attr-defined]
                self._assert_hidden(
                    report + redirect_path.read_text(encoding="utf-8"),
                    _BOUND_TOKEN,
                    "synthetic-redirect-body",
                    target_url,
                )
            finally:
                for running in (origin, target):
                    running.shutdown()
                    running.server_close()

            full = Path(directory) / "full.json"
            _bound_text(full, {"eventId": "evt-seed", "content": "seed"}, "http://127.0.0.1:3111")
            state = json.loads(full.read_text(encoding="utf-8"))
            state["events"] = [
                {
                    "attempts": 0,
                    "lastError": None,
                    "payload": {"content": "queue-fill", "eventId": f"evt-fill-{index}"},
                }
                for index in range(OUTBOX.MAX_EVENTS)
            ]
            full.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            filled = full.read_bytes()
            with self.assertRaises(ValueError) as overflow:
                OUTBOX.enqueue_bound(
                    full,
                    {"eventId": "evt-overflow", "content": f"overflow {SK}"},
                    "http://127.0.0.1:3111",
                    _BOUND_TOKEN,
                )
            self.assertIn("full", str(overflow.exception))
            self.assertEqual(full.read_bytes(), filled)
            self._assert_hidden(str(overflow.exception), SK, _BOUND_TOKEN)

            buffer = io.StringIO()
            with redirect_stdout(buffer):
                self.assertEqual(OUTBOX.status(full), 0)
            status_text = buffer.getvalue()
            self.assertIn('"version": 2', status_text)
            self.assertNotIn(state["deviceFingerprint"], status_text)
            self._assert_hidden(status_text, _BOUND_TOKEN, SK)
            purged = io.StringIO()
            with redirect_stdout(purged):
                self.assertEqual(OUTBOX.purge(full, "evt-fill-0"), 0)
            purge_text = purged.getvalue()
            self.assertNotIn(state["deviceFingerprint"], purge_text)
            self._assert_hidden(purge_text, _BOUND_TOKEN)
            kept = json.loads(full.read_text(encoding="utf-8"))
            self.assertEqual(kept["version"], 2)
            self.assertEqual(kept["deviceFingerprint"], state["deviceFingerprint"])
            self.assertEqual(kept["origin"], "http://127.0.0.1:3111")
            self.assertNotIn("evt-fill-0", [item["payload"]["eventId"] for item in kept["events"]])
            self.assertEqual(len(kept["events"]), OUTBOX.MAX_EVENTS - 1)

    def test_version1_commands_do_not_mix_with_version2(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bound = Path(directory) / "bound.json"
            _bound_text(bound, {"eventId": "evt-v2", "content": f"safe {SK}"}, "http://127.0.0.1:3111")
            before = bound.read_bytes()
            with self.assertRaises(ValueError) as refused:
                enqueue_in_process(bound, {"eventId": "evt-v1", "content": f"mix {SK}"})
            self.assertEqual(str(refused.exception), "outbox file is not a version 1 queue")
            self.assertEqual(bound.read_bytes(), before)
            completed = enqueue_cli(bound, {"eventId": "evt-cli", "content": f"mix {SK}"})
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(bound.read_bytes(), before)
            self._assert_hidden(completed.stdout + completed.stderr, _BOUND_TOKEN, SK, "evt-cli")

            version1 = Path(directory) / "version1.json"
            enqueue_in_process(version1, {"eventId": "evt-v1", "content": "plain"})
            version1_bytes = version1.read_bytes()
            self.assertNotIn(b"deviceFingerprint", version1_bytes)
            with self.assertRaises(ValueError) as bound_refused:
                OUTBOX.enqueue_bound(
                    version1,
                    {"eventId": "evt-add", "content": f"nope {SK}"},
                    "http://127.0.0.1:3111",
                    _BOUND_TOKEN,
                )
            self.assertEqual(str(bound_refused.exception), "outbox file is not a version 2 queue")
            self.assertEqual(version1.read_bytes(), version1_bytes)
            self._assert_hidden(str(bound_refused.exception), SK, _BOUND_TOKEN)

            server, base = serve([])
            try:
                os.environ["CAIRN_ACCOUNT_TOKEN"] = "synthetic-device-token"
                completed_replay = subprocess.run(
                    [
                        sys.executable,
                        str(OUTBOX_PATH),
                        "replay",
                        str(version1),
                        "--url",
                        base,
                        "--token",
                        "synthetic-device-token",
                    ],
                    text=True,
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(completed_replay.returncode, 0, completed_replay.stderr)
                self.assertEqual(json.loads(completed_replay.stdout)["accepted"], 1)
                replayed = json.loads(version1.read_text(encoding="utf-8"))
                self.assertEqual(replayed["version"], 1)
                self.assertNotIn("deviceFingerprint", replayed)
                self.assertEqual(len(server.bodies), 1)  # type: ignore[attr-defined]
            finally:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    unittest.main()
