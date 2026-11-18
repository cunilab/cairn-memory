#!/usr/bin/env python3
"""Hook transport boundary. Stub HTTP plus one isolated file-server check.

No live API. Tokens and payload secrets are asserted in memory and must not
appear in hook stdout or stderr.
"""

import hashlib
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import cairn_outbox


ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / "scripts" / "cairn_hook.py"
ACCOUNT = "account-token-hook7a"
DEVICE = "device-token-hook7a"
SECRET = "api_key=hook-secret-value"


def fingerprint(session_id, content, source):
    encoded = json.dumps(
        {"sessionId": session_id, "content": cairn_outbox.redact_content(content), "source": source},
        sort_keys=True,
    ).encode()
    return "hook-" + hashlib.sha256(encoded).hexdigest()[:24]


def run_hook(url, payload, *, cwd=ROOT, **env):
    child = os.environ.copy()
    for name in ("CAIRN_ACCOUNT_TOKEN", "CAIRN_OUTBOX", "CAIRN_URL", "CAIRN_DEVICE_TOKEN", "CAIRN_CONFIG"):
        child.pop(name, None)
    token = DEVICE
    chosen_url = url
    if "CAIRN_DEVICE_TOKEN" in env:
        token = env.pop("CAIRN_DEVICE_TOKEN")
    if "CAIRN_URL" in env:
        override = env.pop("CAIRN_URL")
        if override is not None:
            chosen_url = override
    for key, value in env.items():
        if value is None:
            child.pop(key, None)
        else:
            child[key] = value
    command = [sys.executable, str(HOOK)]
    with tempfile.TemporaryDirectory() as directory:
        child["XDG_CONFIG_HOME"] = directory
        if isinstance(token, str):
            config = Path(directory) / "config.json"
            config.write_text(json.dumps({"url": chosen_url, "token": token}), encoding="utf-8")
            command.extend(["--config", str(config)])
        if not isinstance(payload, str):
            payload = json.dumps(payload)
        return subprocess.run(
            command,
            input=payload,
            text=True,
            capture_output=True,
            env=child,
            cwd=cwd,
            timeout=20,
            check=False,
        )


def make_repo(parent, name, remote=None, branch="feature/x"):
    """A real git repo with one commit, so HEAD names a branch."""
    path = Path(parent) / name
    path.mkdir()
    env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull)
    git = ["git", "-C", str(path), "-c", "user.name=t", "-c", "user.email=t@example.test"]
    subprocess.run(git + ["init", "-q", "-b", branch], check=True, env=env)
    subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", "x"], check=True, env=env)
    if remote:
        subprocess.run(git + ["remote", "add", "origin", remote], check=True, env=env)
    return path


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        self.server.captured.append(
            {
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
                "user_agent": self.headers.get("User-Agent"),
                "body": body,
            }
        )
        status, payload = self.server.reply
        data = payload.encode()
        self.send_response(status)
        if status in (301, 302, 303, 307, 308):
            self.send_header("Location", "/cairn-memory/sync/push")
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        return


class StubHookTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.server.captured = []
        cls.server.reply = (200, '{"accepted":true}')
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.url = "http://127.0.0.1:%d" % cls.server.server_address[1]

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        self.server.captured = []
        self.server.reply = (200, '{"accepted":true}')

    def assert_hidden(self, result, *secrets):
        combined = result.stdout + result.stderr
        for secret in secrets:
            if secret and secret in combined:
                self.fail("hook output exposed a credential or payload value")

    def assert_bearer(self, header, token):
        if token is None:
            self.assertIsNone(header)
        else:
            self.assertTrue(header == "Bearer " + token)

    def test_assistant_capture_requires_explicit_opt_in(self):
        for payload in (
            {"role": "assistant", "content": "private answer"},
            {"hook_event_name": "Stop", "last_assistant_message": "private answer"},
            {"last_assistant_message": "private answer"},
            {"payload": {"last_assistant_message": "private answer"}},
            {"role": "tool", "payload": {"role": "assistant", "content": "private answer"}},
            {"payload": {"role": "assistant", "content": "private answer"}},
        ):
            with tempfile.TemporaryDirectory() as directory:
                queue = Path(directory) / "queue.json"
                result = run_hook(self.url, dict(payload, sessionId="assistant"),
                                  CAIRN_CAPTURE_ASSISTANT=None, CAIRN_OUTBOX=str(queue))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.server.captured, [])
                self.assertEqual(list(Path(directory).iterdir()), [])
        result = run_hook(self.url, {"sessionId": "assistant", "role": "assistant", "content": "allowed"},
                          CAIRN_CAPTURE_ASSISTANT="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.server.captured), 1)

    def test_skip_marker_stops_at_git_root_and_applies_to_non_git_ancestors(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            repo = make_repo(parent, "repo")
            nested = repo / "src"
            nested.mkdir()
            queue = parent / "queue.json"
            payload = {"sessionId": "skip", "content": "private", "cwd": str(nested)}
            (repo / ".cairnignore").touch()
            result = run_hook(self.url, payload, CAIRN_OUTBOX=str(queue))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.server.captured, [])
            self.assertFalse(queue.exists())
            self.assertFalse(Path(str(queue) + ".lock").exists())
            (repo / ".cairnignore").unlink()
            (parent / ".cairnignore").touch()
            result = run_hook(self.url, payload)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(self.server.captured), 1, "outside the Git root is ignored")
            self.server.captured = []
            plain = parent / "plain"
            plain.mkdir()
            result = run_hook(self.url, dict(payload, cwd=str(plain)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.server.captured, [])
            (parent / ".cairnignore").unlink()
            (nested / ".cairnignore").mkdir()
            result = run_hook(self.url, payload)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(len(self.server.captured), 1, "a directory is not a marker")

    def test_skip_marker_uses_process_cwd_when_payload_has_none(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / ".cairnignore").touch()
            result = run_hook(self.url, {"sessionId": "skip", "content": "private"}, cwd=directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.server.captured, [])

    def test_secrets_are_scrubbed_before_content_and_tool_truncation(self):
        # Redaction first preserves useful text after a secret longer than either cap.
        for payload in (
            {"content": "token=" + "q" * 70000 + " kept-after-secret"},
            {"tool_name": "Bash", "tool_response": "token=" + "q" * 20000 + " kept-after-secret"},
        ):
            self.server.captured = []
            result = run_hook(self.url, dict(payload, sessionId="redact-first"))
            self.assertEqual(result.returncode, 0, result.stderr)
            body = json.loads(self.server.captured[0]["body"])
            self.assertNotIn("q" * 32, body["content"])
            self.assertIn("[REDACTED]", body["content"])
            self.assertIn("kept-after-secret", body["content"])

    def test_missing_device_token_rejects_before_post(self):
        for token in (None, "", "  "):
            result = run_hook(
                self.url,
                {"sessionId": "session-1", "content": SECRET},
                CAIRN_DEVICE_TOKEN=token,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.server.captured, [])
            self.assertIn("cairn setup", result.stderr)
            self.assertNotIn(DEVICE, result.stderr)
            self.assert_hidden(result, SECRET)

    def test_account_token_env_is_ignored_and_never_sent(self):
        result = run_hook(
            self.url,
            {
                "session_id": "snake-session",
                "event_id": "snake-event",
                "text": "plain capture",
                "type": "SnakeHook",
                "role": "assistant",
                "context": {"repository": "cairn"},
            },
            CAIRN_ACCOUNT_TOKEN=ACCOUNT,
            CAIRN_CAPTURE_ASSISTANT="1",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.server.captured), 1)
        posted = self.server.captured[0]
        self.assertEqual(posted["path"], "/cairn-memory/sync/push")
        self.assert_bearer(posted["authorization"], DEVICE)
        self.assertNotIn(ACCOUNT, posted["body"].decode())
        body = json.loads(posted["body"])
        self.assertEqual(body["eventId"], "snake-event")
        self.assertEqual(body["sessionId"], "snake-session")
        self.assertEqual(body["source"], "SnakeHook")
        self.assertEqual(body["role"], "assistant")
        self.assertEqual(body["context"]["repository"], "cairn")
        self.assert_hidden(result, ACCOUNT, DEVICE)

    def test_post_sends_the_cairn_hook_user_agent(self):
        # urllib's default Python-urllib agent is blocked by Cloudflare (error 1010).
        result = run_hook(self.url, {"sessionId": "session-ua", "content": "plain"})
        self.assertEqual(result.returncode, 0, result.stderr)
        agent = self.server.captured[0]["user_agent"]
        self.assertTrue(agent.startswith("cairn-hook/"), agent)

    def test_proxy_block_is_reported_as_a_proxy_rejection(self):
        self.server.reply = (403, "error code: 1010")
        result = run_hook(self.url, {"sessionId": "session-403", "content": "plain"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "rejected by a proxy in front of Cairn (403: error code: 1010)", result.stderr
        )
        self.assert_hidden(result, DEVICE)

    def test_device_token_posts_sync_and_redacts_before_the_request(self):
        content = "decision " + SECRET
        source = "PostToolUse"
        result = run_hook(
            self.url,
            {
                "sessionId": "device-session",
                "content": content,
                "role": "tool",
                "source": source,
                "context": {"repository": "cairn", "commands": ["token=hook-secret-value"]},
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.server.captured), 1)
        posted = self.server.captured[0]
        self.assertEqual(posted["path"], "/cairn-memory/sync/push")
        self.assert_bearer(posted["authorization"], DEVICE)
        body = json.loads(posted["body"])
        self.assertEqual(body["eventId"], fingerprint("device-session", content, source))
        self.assertEqual(body["role"], "tool")
        self.assertEqual(body["source"], source)
        self.assertEqual(body["context"]["repository"], "cairn")
        self.assertNotIn("hook-secret-value", posted["body"].decode())
        self.assertIn("api_key=[REDACTED]", body["content"])
        self.assertEqual(body["context"]["commands"], ["token=[REDACTED]"])
        self.assert_hidden(result, DEVICE, "hook-secret-value")

    def test_base_path_prefixes_the_sync_route_bound_or_not(self):
        for bound in (False, True):
            with self.subTest(bound=bound):
                self.server.captured = []
                with tempfile.TemporaryDirectory() as directory:
                    queue = Path(directory) / "queue.json"
                    result = run_hook(
                        self.url + "/agent/",
                        {"sessionId": "s", "eventId": "base-%s" % bound, "content": "x"},
                        CAIRN_OUTBOX=str(queue) if bound else None,
                    )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(self.server.captured), 1)
                self.assertEqual(self.server.captured[0]["path"], "/agent/cairn-memory/sync/push")

    def test_role_does_not_change_the_event_fingerprint(self):
        payload = {
            "sessionId": "same-session",
            "content": "same content",
            "source": "PostToolUse",
        }
        first = run_hook(self.url, dict(payload, role="tool"))
        second = run_hook(self.url, dict(payload, role="assistant"), CAIRN_CAPTURE_ASSISTANT="1")
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        bodies = [json.loads(item["body"]) for item in self.server.captured]
        self.assertEqual(bodies[0]["eventId"], bodies[1]["eventId"])
        self.assertEqual(bodies[0]["eventId"], fingerprint("same-session", "same content", "PostToolUse"))
        self.assertEqual(bodies[0]["role"], "tool")
        self.assertEqual(bodies[1]["role"], "assistant")
        self.assertEqual({item["path"] for item in self.server.captured}, {"/cairn-memory/sync/push"})

    def test_nested_camel_case_still_normalizes(self):
        result = run_hook(
            self.url,
            {"payload": {"sessionId": "nested", "eventId": "nested-event", "message": "nested text", "source": "Nested"}},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        body = json.loads(self.server.captured[0]["body"])
        self.assertEqual(body["sessionId"], "nested")
        self.assertEqual(body["eventId"], "nested-event")
        self.assertEqual(body["content"], "nested text")
        self.assertEqual(body["source"], "Nested")
        self.assertEqual(body["role"], "tool")

    def test_claude_code_hook_input_maps_to_capture_fields(self):
        common = {"session_id": "cc-session", "cwd": "/work", "transcript_path": "/t.jsonl"}
        cases = [
            ({"hook_event_name": "UserPromptSubmit", "prompt": "fix the bug"}, "fix the bug", "user", "UserPromptSubmit"),
            ({"hook_event_name": "Stop", "last_assistant_message": "done"}, "done", "assistant", "Stop"),
            ({"hook_event_name": "SessionStart", "source": "startup"}, None, "tool", "SessionStart"),
            (
                {
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Bash",
                    "tool_input": {"command": "cargo test"},
                    "tool_response": {"stdout": "x" * 40000},
                },
                None,
                "tool",
                "PostToolUse",
            ),
        ]
        for extra, content, role, source in cases:
            self.server.captured = []
            result = run_hook(self.url, dict(common, **extra), CAIRN_CAPTURE_ASSISTANT="1")
            self.assertEqual(result.returncode, 0, result.stderr)
            body = json.loads(self.server.captured[0]["body"])
            self.assertEqual((body["sessionId"], body["role"], body["source"]), ("cc-session", role, source))
            if content is not None:
                self.assertEqual(body["content"], content)
        self.assertTrue(body["content"].startswith('Bash {"tool_input":{"command":"cargo test"}'))
        self.assertLessEqual(len(body["content"]), 16000)

    def _context_for(self, cwd, **env):
        self.server.captured = []
        payload = {"session_id": "ctx-session", "hook_event_name": "UserPromptSubmit", "prompt": "p", "cwd": str(cwd)}
        self.assertEqual(run_hook(self.url, payload, **env).returncode, 0)
        return json.loads(self.server.captured[0]["body"])

    def test_cwd_in_a_git_repo_fills_repository_and_branch(self):
        with tempfile.TemporaryDirectory() as tmp:
            # The slug is the origin's last path part: not the checkout dir, no
            # credentials, no absolute path.
            repo = make_repo(tmp, "checkout-dir", "https://user:pw-secret@example.test/acme/widget-svc.git")
            (repo / "src").mkdir()
            body = self._context_for(repo / "src")
            self.assertEqual(body["context"], {"repository": "example.test/acme/widget-svc", "branch": "feature/x"})
            self.assertNotIn(tmp, json.dumps(body))
            self.assertNotIn("pw-secret", json.dumps(body))
            # No remote: fall back to the top-level directory name.
            bare = make_repo(tmp, "local-only")
            self.assertEqual(self._context_for(bare)["context"]["repository"], "local:local-only")
            # scp-style remote
            scp = make_repo(tmp, "other", "git@example.test:acme/scp-repo.git")
            self.assertEqual(self._context_for(scp)["context"]["repository"], "example.test/acme/scp-repo")

    def test_derived_context_keeps_event_id_and_explicit_values(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp, "r", "https://example.test/acme/widget-svc.git")
            plain = {"session_id": "ctx-session", "hook_event_name": "UserPromptSubmit", "prompt": "p"}
            self.assertEqual(run_hook(self.url, dict(plain, cwd=str(repo))).returncode, 0)
            self.assertEqual(run_hook(self.url, plain).returncode, 0)
            ids = [json.loads(item["body"])["eventId"] for item in self.server.captured]
            self.assertEqual(ids[0], ids[1])
            # An explicit repository is never replaced, and its other keys stay.
            explicit = dict(plain, cwd=str(repo), context={"repository": "mine", "commands": ["make"]})
            self.server.captured = []
            self.assertEqual(run_hook(self.url, explicit).returncode, 0)
            self.assertEqual(json.loads(self.server.captured[0]["body"])["context"], {"repository": "mine", "commands": ["make"]})
            # A context without a repository is completed from cwd.
            partial = dict(plain, cwd=str(repo), context={"commands": ["make"]})
            self.server.captured = []
            self.assertEqual(run_hook(self.url, partial).returncode, 0)
            self.assertEqual(
                json.loads(self.server.captured[0]["body"])["context"],
                {"repository": "example.test/acme/widget-svc", "branch": "feature/x", "commands": ["make"]},
            )

    def test_project_file_at_the_git_top_level_sends_the_project_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp, "r", "https://example.test/acme/widget-svc.git")
            (repo / "src").mkdir()
            self.assertEqual(self._context_for(repo)["context"], {"repository": "example.test/acme/widget-svc", "branch": "feature/x"})
            (repo / ".cairn").mkdir()
            (repo / ".cairn" / "project.json").write_text(
                json.dumps({"project_id": "project-abc", "repository": "example.test/acme/widget-svc"}), encoding="utf-8"
            )
            # Read from a subdirectory: the file lives at the git top level.
            self.assertEqual(
                self._context_for(repo / "src")["context"],
                {"repository": "example.test/acme/widget-svc", "branch": "feature/x", "project_id": "project-abc"},
            )
            # v0.1.4 adds a display name; capture still uses only the stable id.
            (repo / ".cairn" / "project.json").write_text(
                json.dumps({"project_id": "project-abc", "repository": "example.test/acme/widget-svc", "name": "Widget Service"}), encoding="utf-8"
            )
            self.assertEqual(
                self._context_for(repo / "src")["context"],
                {"repository": "example.test/acme/widget-svc", "branch": "feature/x", "project_id": "project-abc"},
            )
            # An explicit project_id (and repository) in the input is kept.
            explicit = {
                "session_id": "ctx-session",
                "hook_event_name": "UserPromptSubmit",
                "prompt": "p",
                "cwd": str(repo),
                "context": {"repository": "mine", "project_id": "project-mine"},
            }
            self.server.captured = []
            self.assertEqual(run_hook(self.url, explicit).returncode, 0)
            self.assertEqual(
                json.loads(self.server.captured[0]["body"])["context"],
                {"repository": "mine", "project_id": "project-mine"},
            )
            # An explicit repository still picks up the file's id.
            partial = dict(explicit, context={"repository": "mine"})
            self.server.captured = []
            self.assertEqual(run_hook(self.url, partial).returncode, 0)
            self.assertEqual(
                json.loads(self.server.captured[0]["body"])["context"],
                {"repository": "mine", "project_id": "project-abc"},
            )
            # Unreadable or empty files send no id and never fail the hook.
            for body in ("not json", "{}", '{"project_id": ""}', '{"project_id": 7}', "[]"):
                (repo / ".cairn" / "project.json").write_text(body, encoding="utf-8")
                self.assertNotIn("project_id", self._context_for(repo)["context"], body)

    def test_no_context_when_cwd_is_not_a_repo_or_git_is_unavailable(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self._context_for(tmp)["context"], {})
            self.assertEqual(self._context_for(Path(tmp) / "missing")["context"], {})
            repo = make_repo(tmp, "r", "https://example.test/acme/widget-svc.git")
            empty_path = Path(tmp) / "empty-bin"
            empty_path.mkdir()
            self.assertEqual(self._context_for(repo, PATH=str(empty_path))["context"], {})

    def test_detached_head_sends_a_repository_but_no_branch(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp, "r", "https://example.test/acme/widget-svc.git")
            subprocess.run(["git", "-C", str(repo), "checkout", "-q", "--detach"], check=True)
            self.assertEqual(self._context_for(repo)["context"], {"repository": "example.test/acme/widget-svc"})

    def test_spaced_token_rejects_before_post(self):
        bad = "sk-not-a-header token"
        result = run_hook(
            self.url,
            {"sessionId": "s", "content": SECRET},
            CAIRN_DEVICE_TOKEN=bad,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.server.captured, [])
        self.assertIn("one header token", result.stderr)
        self.assert_hidden(result, bad, SECRET, "sk-not-a-header")

    def test_malformed_input_hides_the_value_and_the_token(self):
        result = run_hook(self.url, '{"content": "' + SECRET)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.server.captured, [])
        self.assertIn("JSON object", result.stderr)
        self.assert_hidden(result, DEVICE, SECRET)

    def test_bad_device_context_rejects_before_post(self):
        result = run_hook(
            self.url,
            {"sessionId": "s", "content": SECRET, "context": {"files": [1]}},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.server.captured, [])
        self.assertIn("list of strings", result.stderr)
        self.assert_hidden(result, DEVICE, SECRET)

    def test_redirect_is_not_followed(self):
        self.server.reply = (302, "{}")
        result = run_hook(
            self.url,
            {"sessionId": "s", "eventId": "redirect-event", "content": SECRET},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.server.captured), 1)
        self.assertEqual(self.server.captured[0]["path"], "/cairn-memory/sync/push")
        self.assert_bearer(self.server.captured[0]["authorization"], DEVICE)
        self.assert_hidden(result, DEVICE, SECRET)

    def proxy_env(self, proxy_url):
        return {
            "HTTP_PROXY": proxy_url,
            "http_proxy": proxy_url,
            "NO_PROXY": None,
            "no_proxy": None,
        }

    def test_plain_http_off_loopback_is_refused_before_any_connection(self):
        # The stub doubles as an HTTP proxy: any request that leaves the hook
        # for a non-loopback http host would reach it and be captured.
        urls = (
            "http://example.invalid:3111",
            "http://10.255.255.1:3111",
            "http://localhost.example.invalid",
            "http://127.0.0.1.example.invalid",
            "http://user:pw-hook7a@127.0.0.1:3111",
            "http://127.0.0.1:3111?token=query-hook7a",
        )
        for url in urls:
            for bound in (False, True):
                with self.subTest(url=url.split("@")[-1], bound=bound):
                    with tempfile.TemporaryDirectory() as directory:
                        queue = Path(directory) / "queue.json"
                        env = self.proxy_env(self.url)
                        env["CAIRN_OUTBOX"] = str(queue) if bound else None
                        result = run_hook(
                            url,
                            {"sessionId": "s", "eventId": "plain-http", "content": SECRET},
                            **env,
                        )
                        self.assertNotEqual(result.returncode, 0)
                        self.assertEqual(self.server.captured, [])
                        self.assertFalse(queue.exists())
                        self.assertNotIn("Traceback", result.stderr)
                        self.assert_hidden(result, DEVICE, SECRET, "pw-hook7a", "query-hook7a")
        refused = run_hook(
            "http://example.invalid:3111",
            {"sessionId": "s", "content": SECRET},
        )
        self.assertIn("loopback", refused.stderr)

    def test_https_non_loopback_passes_the_url_check(self):
        # The refusal is the scheme rule only. https to a closed port fails at
        # connect, not at the URL check.
        result = run_hook(
            "https://127.0.0.1:9",
            {"sessionId": "s", "content": SECRET},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("loopback", result.stderr)
        self.assertIn("delivery failed", result.stderr)

    def test_loopback_http_never_goes_through_the_proxy(self):
        proxy = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        proxy.captured = []
        proxy.reply = (200, '{"accepted":true}')
        thread = threading.Thread(target=proxy.serve_forever, daemon=True)
        thread.start()
        try:
            proxy_url = "http://127.0.0.1:%d" % proxy.server_address[1]
            for bound in (False, True):
                with self.subTest(bound=bound):
                    self.server.captured = []
                    with tempfile.TemporaryDirectory() as directory:
                        env = self.proxy_env(proxy_url)
                        env["CAIRN_OUTBOX"] = str(Path(directory) / "queue.json") if bound else None
                        result = run_hook(
                            self.url,
                            {"sessionId": "s", "eventId": "loopback-%s" % bound, "content": "x"},
                            **env,
                        )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(proxy.captured, [])
                    self.assertEqual(len(self.server.captured), 1)
                    self.assert_bearer(self.server.captured[0]["authorization"], DEVICE)
        finally:
            proxy.shutdown()
            proxy.server_close()
            thread.join(timeout=5)

    def test_connection_failure_drops_the_capture_without_a_traceback(self):
        result = run_hook(
            "http://127.0.0.1:9",
            {"sessionId": "s", "content": SECRET},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Traceback", result.stderr)
        self.assertIn("delivery failed", result.stderr)
        self.assert_hidden(result, DEVICE, SECRET)

    def test_oversized_content_is_truncated_with_a_marker_not_rejected(self):
        # Multibyte text: the cut must land on a character boundary.
        result = run_hook(
            self.url,
            {"sessionId": "s", "hook_event_name": "UserPromptSubmit", "prompt": "\u00e9" * (64 * 1024)},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        body = json.loads(self.server.captured[0]["body"])
        self.assertTrue(body["content"].endswith("[truncated]"))
        self.assertLessEqual(len(body["content"].encode()), 64 * 1024)
        self.assert_hidden(result, DEVICE)

    def test_tool_use_id_keeps_identical_tool_results_apart(self):
        base = {"session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "mkdir hook-scratch"}}
        for tool_use_id in ("toolu_1", "toolu_2", "toolu_2", None):
            extra = {} if tool_use_id is None else {"tool_use_id": tool_use_id}
            self.assertEqual(run_hook(self.url, dict(base, **extra)).returncode, 0)
        ids = [json.loads(item["body"])["eventId"] for item in self.server.captured]
        self.assertEqual(len(set(ids[:2] + ids[3:])), 3)
        self.assertEqual(ids[1], ids[2])

    def test_read_only_tools_are_not_delivered_and_mutating_commands_are(self):
        skipped = (
            {"tool_name": "Read", "tool_input": {"file_path": "src/lib.rs"}},
            {"tool_name": "Bash", "tool_input": {"command": "ls src"}},
            {"tool_name": "Bash", "tool_input": {"command": "git status --short"}},
            {"tool_name": "Bash", "tool_input": {"command": "ls && rm -rf scratch"}},
        )
        # The last command is mutating because of `&&`; it must be delivered.
        for index, extra in enumerate(skipped):
            payload = {"session_id": "s", "hook_event_name": "PostToolUse", **extra}
            self.assertEqual(run_hook(self.url, payload).returncode, 0)
            if index < 3:
                self.assertEqual(self.server.captured, [])
            else:
                self.assertEqual(len(self.server.captured), 1)
            self.server.captured.clear()
        ambiguous = {"session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "FOO=1 ls"}}
        self.assertEqual(run_hook(self.url, ambiguous).returncode, 0)
        self.assertEqual(len(self.server.captured), 1)

    def test_tool_excerpt_is_redacted_before_the_two_kilobyte_clip(self):
        secret = "sk-synthetic-hook-secret"
        payload = {
            "session_id": "s",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "mkdir hook-scratch"},
            "tool_response": secret + " " + ("é" * 3000) + " tail-marker",
        }
        self.assertEqual(run_hook(self.url, payload).returncode, 0)
        body = json.loads(self.server.captured[0]["body"])
        self.assertNotIn(secret, body["content"])
        self.assertIn("[redacted]", body["content"].lower())
        self.assertLessEqual(len(body["content"].encode()), 2048)
        self.assertTrue(body["content"].endswith("[truncated]"))
        body["content"].encode("utf-8")

    def test_nested_mutating_tool_is_capped_after_redaction_and_replays(self):
        secret = "sk-synthetic-nested-secret"
        payload = {
            "session_id": "s",
            "hook_event_name": "PostToolUse",
            "payload": {
                "tool_name": "Bash",
                "tool_input": {"command": "mkdir nested-scratch"},
                "tool_response": secret + " " + ("é" * 3000),
            },
        }
        self.assertEqual(run_hook(self.url, payload).returncode, 0)
        self.assertEqual(run_hook(self.url, payload).returncode, 0)
        bodies = [json.loads(item["body"]) for item in self.server.captured]
        self.assertEqual(len(bodies), 2)
        self.assertEqual(bodies[0]["eventId"], bodies[1]["eventId"])
        content = bodies[0]["content"]
        self.assertTrue(content.startswith("Bash {"))
        self.assertNotIn(secret, content)
        self.assertIn("[redacted]", content.lower())
        self.assertLessEqual(len(content.encode()), 2048)
        self.assertTrue(content.endswith("[truncated]"))
        content.encode("utf-8")

    def _invoke(self, payload, env, args):
        child = os.environ.copy()
        for name in ("CAIRN_ACCOUNT_TOKEN", "CAIRN_OUTBOX", "CAIRN_URL", "CAIRN_DEVICE_TOKEN", "CAIRN_CONFIG"):
            child.pop(name, None)
        child.update(env)
        return subprocess.run(
            [sys.executable, str(HOOK), *args],
            input=json.dumps(payload),
            text=True,
            capture_output=True,
            env=child,
            cwd=ROOT,
            timeout=20,
            check=False,
        )

    def test_legacy_env_cannot_override_a_valid_config(self):
        payload = {
            "sessionId": "legacy",
            "eventId": "legacy-event",
            "content": "kept",
            "role": "tool",
        }
        with tempfile.TemporaryDirectory() as directory:
            good = Path(directory) / "good.json"
            good.write_text(json.dumps({"url": self.url, "token": DEVICE}), encoding="utf-8")
            cases = (
                {"CAIRN_URL": "http://127.0.0.1:9", "CAIRN_DEVICE_TOKEN": "legacy-token-value"},
                {"CAIRN_URL": "http://127.0.0.1:9"},
                {"CAIRN_DEVICE_TOKEN": "legacy-token-value"},
                {"CAIRN_URL": ""},
                {"CAIRN_DEVICE_TOKEN": "   "},
            )
            for extra in cases:
                with self.subTest(extra=tuple(extra)):
                    self.server.captured = []
                    result = self._invoke(payload, extra, ["--config", str(good)])
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(self.server.captured[0]["authorization"], "Bearer " + DEVICE)
                    self.assertIn("ignoring", result.stderr)
                    self.assertNotIn("legacy-token-value", result.stdout + result.stderr)
                    self.assertNotIn(DEVICE, result.stdout + result.stderr)
                    self.assertNotIn("127.0.0.1:9", result.stderr)

    def test_cairn_config_default_and_explicit_file_precedence(self):
        payload = {
            "sessionId": "select",
            "eventId": "select-event",
            "content": "kept",
            "role": "tool",
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "good.json"
            decoy = root / "decoy.json"
            default = root / "cairn" / "config.json"
            default.parent.mkdir()
            good.write_text(json.dumps({"url": self.url, "token": DEVICE}), encoding="utf-8")
            decoy.write_text(
                json.dumps({"url": "http://127.0.0.1:9", "token": "decoy-token-value"}),
                encoding="utf-8",
            )
            default.write_text(
                json.dumps({"url": "http://127.0.0.1:9", "token": "decoy-token-value"}),
                encoding="utf-8",
            )
            selected = self._invoke(
                payload,
                {"CAIRN_CONFIG": str(good), "XDG_CONFIG_HOME": str(root)},
                [],
            )
            self.assertEqual(selected.returncode, 0, selected.stderr)
            self.assertEqual(self.server.captured[-1]["authorization"], "Bearer " + DEVICE)
            explicit = self._invoke(
                payload,
                {"CAIRN_CONFIG": str(decoy), "XDG_CONFIG_HOME": str(root)},
                ["--config", str(good)],
            )
            self.assertEqual(explicit.returncode, 0, explicit.stderr)
            self.assertEqual(self.server.captured[-1]["authorization"], "Bearer " + DEVICE)
            self.assertNotIn("decoy-token-value", explicit.stdout + explicit.stderr)
            default.write_text(json.dumps({"url": self.url, "token": DEVICE}), encoding="utf-8")
            plain = self._invoke(payload, {"XDG_CONFIG_HOME": str(root)}, [])
            self.assertEqual(plain.returncode, 0, plain.stderr)
            self.assertEqual(self.server.captured[-1]["authorization"], "Bearer " + DEVICE)
            self.assertNotIn(DEVICE, plain.stdout + plain.stderr)

    def test_http_401_names_the_config_and_leaves_stdout_empty(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.json"
            config.write_text(json.dumps({"url": self.url, "token": DEVICE}), encoding="utf-8")
            self.server.reply = (401, json.dumps({"error": "rejected " + DEVICE}))
            result = self._invoke(
                {
                    "sessionId": "denied",
                    "eventId": "denied-event",
                    "content": "kept",
                    "role": "tool",
                },
                {
                    "CAIRN_URL": "http://127.0.0.1:9",
                    "CAIRN_DEVICE_TOKEN": "legacy-token-value",
                },
                ["--config", str(config)],
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            self.assertIn("rejected hook payload (401)", result.stderr)
            self.assertIn(str(config), result.stderr)
            self.assertIn("cairn setup <server-url>", result.stderr)
            self.assertIn("set CAIRN_CONFIG to that file", result.stderr)
            self.assertIn("[REDACTED]", result.stderr)
            self.assertNotIn(DEVICE, result.stdout + result.stderr)
            self.assertNotIn("legacy-token-value", result.stdout + result.stderr)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_json(url, path, payload=None, token=None):
    data = None if payload is None else json.dumps(payload).encode()
    headers = {"Content-Type": "application/json"} if data is not None else {}
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(
        url + path,
        data=data,
        headers=headers,
        method="POST" if data is not None else "GET",
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read().decode())
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode(errors="replace")


class FileServerHookTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.store = tempfile.NamedTemporaryFile(prefix="cairn-hook7a-", delete=False)
        cls.store.close()
        os.unlink(cls.store.name)
        cls.log = tempfile.NamedTemporaryFile(prefix="cairn-hook7a-log-", delete=False)
        cls.port = free_port()
        cls.url = "http://127.0.0.1:%d" % cls.port
        subprocess.run(["cargo", "build", "-q", "-p", "cairn-server"], cwd=ROOT, check=True)
        cls.server = subprocess.Popen(
            [
                "cargo",
                "run",
                "-q",
                "-p",
                "cairn-server",
                "--",
                "--store",
                cls.store.name,
                "--bind",
                "127.0.0.1:%d" % cls.port,
            ],
            cwd=ROOT,
            stdout=cls.log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        ready = False
        for _ in range(600):
            if cls.server.poll() is not None:
                break
            try:
                http_json(cls.url, "/cairn-memory/health")
                ready = True
                break
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                time.sleep(0.1)
        if not ready:
            log_text = Path(cls.log.name).read_text(encoding="utf-8", errors="replace")[-4000:]
            cls.tearDownClass()
            raise RuntimeError("file server did not become ready\n" + log_text)
        try:
            status, account = http_json(
                cls.url,
                "/cairn-memory/auth/bootstrap",
                {"email": "hook7a-owner@example.test", "password": "correct horse battery staple"},
            )
            if status != 200:
                raise RuntimeError("bootstrap failed with status %s" % status)
            cls.account_token = account["token"]
            status, device = http_json(
                cls.url,
                "/cairn-memory/devices/register",
                {"name": "Hook7a"},
                cls.account_token,
            )
            if status != 200:
                raise RuntimeError("device register failed with status %s" % status)
            cls.device_token = device["token"]
            cls.device_id = device["device"]["device_id"]
        except Exception:
            cls.tearDownClass()
            raise

    @classmethod
    def tearDownClass(cls):
        server = getattr(cls, "server", None)
        if server is not None and server.poll() is None:
            os.killpg(server.pid, 15)
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(server.pid, 9)
                server.wait(timeout=5)
        for handle in (getattr(cls, "log", None),):
            if handle is not None:
                handle.close()
                os.unlink(handle.name)
        store = getattr(cls, "store", None)
        if store is not None:
            os.unlink(store.name) if os.path.exists(store.name) else None

    def assert_hidden(self, result, *secrets):
        combined = result.stdout + result.stderr
        for secret in secrets:
            if secret and secret in combined:
                self.fail("hook output exposed a credential or payload value")

    def test_device_sync_succeeds_and_other_credentials_do_not_authenticate(self):
        wrong_device_slot = run_hook(
            self.url,
            {
                "sessionId": "hook7a-session",
                "eventId": "hook7a-wrong-device-slot",
                "content": "wrong device slot marker hook7a",
                "role": "tool",
                "source": "PostToolUse",
            },
            CAIRN_DEVICE_TOKEN=self.account_token,
        )
        self.assertNotEqual(wrong_device_slot.returncode, 0)
        self.assertEqual(wrong_device_slot.stdout, "")
        self.assertIn("401", wrong_device_slot.stderr)
        self.assert_hidden(wrong_device_slot, self.account_token, self.device_token)

        device = run_hook(
            self.url,
            {
                "sessionId": "hook7a-session",
                "eventId": "hook7a-device",
                "content": "device sync marker hook7a " + SECRET,
                "role": "tool",
                "source": "PostToolUse",
                "context": {"commands": ["token=hook-secret-value"]},
            },
            CAIRN_DEVICE_TOKEN=self.device_token,
        )
        self.assertEqual(device.returncode, 0, device.stderr)
        pushed = json.loads(device.stdout)
        self.assertTrue(pushed["accepted"] is True)
        self.assertEqual(pushed["observation"]["event_id"], "hook7a-device")
        self.assertNotIn("hook-secret-value", device.stdout)
        self.assert_hidden(device, self.account_token, self.device_token, "hook-secret-value")

        # A checkout without a file stays personal; explicit initialization enables attachment.
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp, "co", "https://example.test/acme/hook7a-repo.git")
            auto = run_hook(
                self.url,
                {
                    "session_id": "hook7a-cwd-session",
                    "hook_event_name": "UserPromptSubmit",
                    "prompt": "cwd derived context marker hook7a",
                    "cwd": str(repo),
                },
                CAIRN_DEVICE_TOKEN=self.device_token,
            )
            self.assertEqual(auto.returncode, 0, auto.stderr)
            self.assertEqual(json.loads(auto.stdout)["project_attachment"]["status"], "personal")
            self.assertEqual(http_json(self.url, "/cairn-memory/projects", None, self.account_token)[1], [])
            status, initialized = http_json(self.url, "/cairn-memory/projects/init", {"repository":"example.test/acme/hook7a-repo"}, self.account_token)
            self.assertEqual(status, 200)
            (repo / ".cairn").mkdir()
            (repo / ".cairn" / "project.json").write_text(json.dumps({"project_id":initialized["project_id"],"repository":"example.test/acme/hook7a-repo"}),encoding="utf-8")
            attached = run_hook(self.url,{"sessionId":"hook7a-attached","eventId":"hook7a-attached-event","content":"initialized capture","cwd":str(repo)},CAIRN_DEVICE_TOKEN=self.device_token)
            self.assertEqual(attached.returncode,0,attached.stderr)
            self.assertEqual(json.loads(attached.stdout)["project_attachment"]["status"],"attached")
            (repo / ".cairn" / "project.json").write_text("broken",encoding="utf-8")
            broken=run_hook(self.url,{"sessionId":"hook7a-broken","eventId":"hook7a-broken-event","content":"personal despite broken file","cwd":str(repo)},CAIRN_DEVICE_TOKEN=self.device_token)
            self.assertEqual(broken.returncode,0,broken.stderr)
            self.assertIn("warning",broken.stderr.lower())
            self.assertEqual(json.loads(broken.stdout)["project_attachment"]["status"],"personal")
            self.assertIn("project.json", json.loads(broken.stdout)["project_attachment"]["warning"])

        # The web session revokes the device; the hook is then rejected.
        status, _ = http_json(
            self.url,
            "/cairn-memory/devices/revoke",
            {"deviceId": self.device_id},
            self.account_token,
        )
        self.assertEqual(status, 200)
        revoked = run_hook(
            self.url,
            {
                "sessionId": "hook7a-session",
                "eventId": "hook7a-revoked",
                "content": "revoked marker hook7a",
                "role": "tool",
                "source": "PostToolUse",
            },
            CAIRN_DEVICE_TOKEN=self.device_token,
        )
        self.assertNotEqual(revoked.returncode, 0)
        self.assertEqual(revoked.stdout, "")
        self.assertIn("401", revoked.stderr)
        self.assert_hidden(revoked, self.account_token, self.device_token)

        with open(self.store.name, encoding="utf-8") as handle:
            state = json.load(handle)
        found = {
            item.get("event_id"): item.get("content", "")
            for item in state.get("observations", [])
        }
        ids = set(found)
        project = [item for item in state["projects"] if item["repository"] == "example.test/acme/hook7a-repo"]
        self.assertEqual(len(project), 1)
        self.assertEqual([item["name"] for item in state["project_branches"]], ["feature/x"])
        self.assertIn("hook7a-device", ids)
        self.assertNotIn("hook7a-wrong-device-slot", ids)
        self.assertNotIn("hook7a-revoked", ids)
        if "hook-secret-value" in found.get("hook7a-device", ""):
            self.fail("device observation retained the raw secret")


class ReadonlyClassificationTests(unittest.TestCase):
    def test_shell_flags_stay_narrow_without_running_commands(self):
        import cairn_hook

        safe = (
            "ls",
            "ls src",
            "git status --short",
            "rg pattern",
            "rg -n pattern src",
            "date",
        )
        unsafe = (
            "rg --pre=helper pattern",
            "rg --pre helper pattern",
            "git diff --ext-diff",
            "date -s 2020-01-01",
            "ls -la",
        )
        for command in safe:
            self.assertTrue(cairn_hook.shell_command_is_readonly(command), command)
        for command in unsafe:
            self.assertFalse(cairn_hook.shell_command_is_readonly(command), command)


if __name__ == "__main__":
    unittest.main()
