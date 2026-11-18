#!/usr/bin/env python3
"""Send one hook payload to Cairn device sync.

CAIRN_DEVICE_TOKEN is required. It posts /cairn-memory/sync/push with a device
bearer; create the device in the web console. CAIRN_OUTBOX queues only a
retryable failure. There is no replay worker. With CAIRN_OUTBOX set, a
delivered event is followed by one bounded, nonblocking replay of that queue
(REPLAY_LIMIT events, REPLAY_BUDGET seconds).
"""

import contextlib
import io
import sqlite3
import argparse
import hashlib
import http.client
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

import cairn_outbox


DEVICE_ENV = "CAIRN_DEVICE_TOKEN"
OUTBOX_ENV = "CAIRN_OUTBOX"
SYNC_PATH = "/cairn-memory/sync/push"
QUEUED_MESSAGE = "Cairn hook queued for replay"
QUEUE_FAILED = "Cairn hook queue failed"
QUEUE_BUSY = "Cairn hook queue is busy; the capture was not retained"
REPLAY_LIMIT = 20
REPLAY_BUDGET = 2.0
# Claude Code hook events (hook_event_name) whose role is not the default "tool".
HOST_ROLES = {"UserPromptSubmit": "user", "Stop": "assistant"}
# shortcut: a plain character cut, 4 bytes each stays under the 64 KiB content
# limit; upgrade if tool output needs smarter summarizing.
TOOL_CONTENT_CHARS = 16000
TRUNCATED_MARKER = "\n[truncated]"


class _Retryable(Exception):
    pass


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Returning None lets urllib keep the 302. Raising keeps the bearer
        # on this route instead of posting it to the redirect target.
        raise urllib.error.HTTPError(req.full_url, code, msg, headers, fp)


_OPENER = urllib.request.build_opener(_NoRedirect)
# Plain http only reaches loopback, so the token never goes through HTTP_PROXY.
_LOOPBACK_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect)


def first(mapping, *keys):
    for key in keys:
        value = mapping.get(key)
        if value not in (None, ""):
            return value
    return None


def _present(name):
    value = os.environ.get(name)
    if value is None or value.strip() == "":
        return None
    return value


def _header_token(name, value):
    if any(character.isspace() or ord(character) < 33 or ord(character) == 127 for character in value):
        raise SystemExit(f"{name} must be one header token")
    return value


def device_token():
    device = _present(DEVICE_ENV)
    if not device:
        raise SystemExit(f"{DEVICE_ENV} is required; create a device in the web console")
    return _header_token(DEVICE_ENV, device)


def scrub(text, *needles):
    for needle in needles:
        if needle:
            text = text.replace(needle, "[REDACTED]")
    return text


def read_input():
    try:
        raw = sys.stdin.buffer.read().decode("utf-8")
    except UnicodeDecodeError:
        raise SystemExit("hook input must be one JSON object") from None
    if not raw.strip():
        raise SystemExit("hook input must be one JSON object")
    try:
        source = json.loads(raw)
    except json.JSONDecodeError:
        raise SystemExit("hook input must be one JSON object") from None
    if not isinstance(source, dict):
        raise SystemExit("hook input must be one JSON object")
    return raw, source


def _git(cwd, *args):
    try:
        done = subprocess.run(
            ["git", "-C", cwd, *args],
            capture_output=True,
            text=True,
            timeout=1,
            check=False,
            stdin=subprocess.DEVNULL,
        )
    except (OSError, subprocess.SubprocessError, ValueError):
        return ""
    return done.stdout.strip() if done.returncode == 0 else ""


def git_context(cwd):
    # Claude Code sends no repository or branch, only cwd. The repository is the
    # last path part of origin (the same short name `project-register` and the
    # examples use; stable across clones and worktrees, no host, user, or
    # credentials), else the top-level directory name. Failure leaves it empty.
    if not isinstance(cwd, str) or not cwd:
        return {}
    top = _git(cwd, "rev-parse", "--show-toplevel")
    if not top:
        return {}
    origin = _git(cwd, "config", "--get", "remote.origin.url").rstrip("/")
    remote = origin.removesuffix(".git")
    if "://" in remote:
        scheme, remote = remote.split("://", 1)
        host, _, path = remote.partition("/")
        if scheme not in ("ssh", "https", "http", "git"):
            host, path = "", ""
    else:
        host, _, path = remote.partition(":")
    host = host.rsplit("@", 1)[-1].split(":", 1)[0].lower()
    name = f"{host}/{path.strip('/')}" if host and path else f"local:{os.path.basename(top)}"
    derived = {"repository": name}
    branch = _git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    if branch and branch != "HEAD":
        derived["branch"] = branch
    return derived


def project_file_id(cwd):
    # The committed project ID joins the capturer; a broken file keeps captures personal.
    if not isinstance(cwd, str) or not cwd:
        return None, None
    top = _git(cwd, "rev-parse", "--show-toplevel")
    if not top:
        return None, None
    try:
        with open(os.path.join(top, ".cairn", "project.json"), encoding="utf-8") as handle:
            value = json.load(handle)
    except FileNotFoundError:
        return None, None
    except (OSError, ValueError):
        return None, "cannot read valid .cairn/project.json; capture remains personal"
    project_id = value.get("project_id") if isinstance(value, dict) else None
    if isinstance(project_id, str) and project_id.strip():
        return project_id.strip(), None
    return None, "invalid .cairn/project.json project id; capture remains personal"


def skip_capture(source, *, check_assistant=True):
    nested = source.get("payload") if isinstance(source.get("payload"), dict) else {}
    assistant = (
        source.get("hook_event_name") == "Stop"
        or any(str(value.get("role")).lower() == "assistant" or "last_assistant_message" in value
               for value in (source, nested))
    )
    if check_assistant and assistant and os.environ.get("CAIRN_CAPTURE_ASSISTANT") != "1":
        return True
    cwd = first(source, "cwd") or first(nested, "cwd") or os.getcwd()
    if not isinstance(cwd, str):
        raise SystemExit("hook cwd must be a folder path")
    folder = Path(cwd).resolve()
    top = _git(str(folder), "rev-parse", "--show-toplevel")
    boundary = Path(top).resolve() if top else None
    for parent in (folder, *folder.parents):
        if (parent / ".cairnignore").is_file():
            return True
        if parent == boundary:
            break
    return False


def build_request(source):
    nested = source.get("payload") if isinstance(source.get("payload"), dict) else {}
    session_id = first(source, "sessionId", "session_id") or first(nested, "sessionId", "session_id") or os.environ.get("CAIRN_SESSION_ID")
    if not session_id:
        raise SystemExit("hook payload requires sessionId or CAIRN_SESSION_ID")
    content = first(source, "content", "text", "message", "prompt", "last_assistant_message") or first(nested, "content", "text", "message")
    if not content and source.get("tool_name"):
        detail = json.dumps({"tool_input": source.get("tool_input"), "tool_response": source.get("tool_response")}, sort_keys=True, separators=(",", ":"))
        content = cairn_outbox.redact_content(f"{source['tool_name']} {detail}")[:TOOL_CONTENT_CHARS]
    if not content:
        content = json.dumps(source, sort_keys=True, separators=(",", ":"))
    content = cairn_outbox.redact_content(str(content))
    if len(content.encode()) > cairn_outbox.MAX_CONTENT_BYTES:
        keep = cairn_outbox.MAX_CONTENT_BYTES - len(TRUNCATED_MARKER)
        content = content.encode()[:keep].decode(errors="ignore") + TRUNCATED_MARKER
    host_event = first(source, "hook_event_name")
    role = first(source, "role") or first(nested, "role") or HOST_ROLES.get(host_event) or "tool"
    source_name = first(source, "hook_event_name", "source", "hook", "type") or first(nested, "source", "type") or "hook"
    event_id = first(source, "eventId", "event_id") or first(nested, "eventId", "event_id")
    if not event_id:
        parts = {"sessionId": session_id, "content": content, "source": source_name}
        tool_use_id = first(source, "tool_use_id")
        if tool_use_id:
            parts["toolUseId"] = str(tool_use_id)
        fingerprint = json.dumps(parts, sort_keys=True).encode()
        event_id = "hook-" + hashlib.sha256(fingerprint).hexdigest()[:24]
    context = first(source, "context") or first(nested, "context") or {}
    cwd = first(source, "cwd") or first(nested, "cwd")
    if isinstance(context, dict) and not context.get("repository"):
        context = {**git_context(cwd), **context}
    attachment_warning = None
    if isinstance(context, dict) and not context.get("project_id"):
        project_id, attachment_warning = project_file_id(cwd)
        if attachment_warning:
            print("Cairn warning: " + attachment_warning, file=sys.stderr)
        if project_id:
            context = {**context, "project_id": project_id}
    lifecycle = first(source, "lifecycle") or first(nested, "lifecycle") or {}
    request = {
        "eventId": event_id,
        "sessionId": session_id,
        "content": str(content),
        "role": str(role),
        "source": str(source_name),
        "context": context if isinstance(context, dict) else {},
        "lifecycle": lifecycle if isinstance(lifecycle, dict) else {},
    }
    importance = first(source, "importance") or first(nested, "importance")
    if importance is not None:
        request["importance"] = importance
    return request, str(content), attachment_warning


def prepare_body(request, token, content, raw):
    try:
        return cairn_outbox.normalize(request)
    except ValueError as error:
        raise SystemExit(scrub(str(error), token, content, raw)) from None


def _retryable_status(code):
    return code == 408 or code == 429 or code >= 500


def _queue_failure(error, token, content, raw):
    if isinstance(error, RuntimeError) and str(error) == cairn_outbox._BUSY_MESSAGE:
        text = QUEUE_BUSY
    elif isinstance(error, RuntimeError) and str(error) == cairn_outbox._LOCK_MESSAGE:
        text = cairn_outbox._LOCK_MESSAGE
    elif isinstance(error, ValueError):
        text = str(error)
    else:
        text = QUEUE_FAILED
    needles = [token]
    if len(content) >= 16:
        needles.append(content)
    if len(raw) >= 16:
        needles.append(raw)
    raise SystemExit(scrub(text, *needles)) from None


def _bound_origin():
    raw = os.environ.get("CAIRN_URL", "http://127.0.0.1:3111")
    try:
        return cairn_outbox.normalize_origin(raw)
    except ValueError as error:
        raise SystemExit(str(error)) from None


def _preflight(spool, origin, token, content, raw):
    try:
        cairn_outbox.preflight_bound(spool, origin, token, nonblocking=True)
    except (OSError, RuntimeError, ValueError) as error:
        if isinstance(error, RuntimeError) and str(error) == cairn_outbox._BUSY_MESSAGE:
            # A replay pass or another hook holds the lock. Still deliver:
            # enqueue rechecks the binding if the post needs a retry.
            return
        _queue_failure(error, token, content, raw)


def _enqueue(spool, origin, token, request, content, raw):
    try:
        cairn_outbox.enqueue_bound(
            spool,
            request,
            origin,
            token,
            nonblocking=True,
            report=False,
        )
    except (OSError, RuntimeError, ValueError) as error:
        _queue_failure(error, token, content, raw)
    sys.stderr.write(QUEUED_MESSAGE + "\n")


def _replay_pending(spool, origin, token, event_id):
    # Best effort after a delivered event. A busy lock, an outage, or any
    # failure here must not change the exit status or the output above.
    try:
        cairn_outbox.replay_bounded(
            spool,
            origin,
            token,
            limit=REPLAY_LIMIT,
            budget=REPLAY_BUDGET,
            skip_event_id=event_id,
        )
    except Exception:
        pass


def _give_up(error, token, content, raw):
    message = scrub(error.read().decode(errors="replace"), token, content, raw)
    if error.code == 403 and not message.lstrip().startswith("{"):
        # Cairn answers JSON; a plain-text 403 comes from a proxy such as Cloudflare.
        raise SystemExit(
            f"Cairn hook rejected by a proxy in front of Cairn (403: {message})"
        ) from None
    raise SystemExit(f"Cairn rejected hook payload ({error.code}): {message}") from None


def _unbound_base():
    base_url = os.environ.get("CAIRN_URL", "http://127.0.0.1:3111").rstrip("/")
    try:
        cairn_outbox.require_secure_url(base_url)
    except ValueError as error:
        raise SystemExit(str(error)) from None
    return base_url


def deliver(token, request, content, raw, base_url, queue_retryable, attachment_warning):
    opener = _LOOPBACK_OPENER if base_url[:5].lower() == "http:" else _OPENER
    headers = {
        "Content-Type": "application/json",
        "Authorization": "Bearer " + token,
        "User-Agent": cairn_outbox.USER_AGENT,
    }
    http_request = urllib.request.Request(
        base_url + SYNC_PATH,
        data=json.dumps(request).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with opener.open(http_request, timeout=10) as response:
            payload = response.read()
    except urllib.error.HTTPError as error:
        if queue_retryable and _retryable_status(error.code):
            error.close()
            raise _Retryable from None
        # A 401 or other 4xx is a real rejection, not a missing queue.
        _give_up(error, token, content, raw)
    except urllib.error.URLError as error:
        if queue_retryable:
            raise _Retryable from None
        reason = scrub(str(error.reason), token, content, raw)
        raise SystemExit(f"Cairn hook delivery failed ({reason})") from None
    except (OSError, http.client.HTTPException):
        if queue_retryable:
            raise _Retryable from None
        raise SystemExit("Cairn hook delivery failed (timed out)") from None
    try:
        response_body = json.loads(payload)
        if attachment_warning:
            response_body["project_attachment"] = {"status": "personal", "warning": attachment_warning}
            payload = json.dumps(response_body).encode()
        attachment = response_body.get("project_attachment") or {}
        if attachment.get("warning") and not attachment_warning:
            sys.stderr.write("Cairn warning: " + scrub(str(attachment["warning"]), token, content, raw) + "\n")
    except (ValueError, AttributeError, TypeError):
        pass
    sys.stdout.write(scrub(payload.decode(), token))
    sys.stdout.write("\n")


# Byte limits are conservative token ceilings, without a tokenizer dependency.
START_NOTE_MAX_BYTES = 256
INSTRUCTIONS_MAX_BYTES = 2048
END_ASK_MAX_BYTES = 2304
BRIEF_MAX_BYTES = 4096
SHORT_NOTE = "Cairn is available: use its memory tools to recall evidence and save reusable knowledge."
WORK_TOOLS = {"Bash", "Edit", "Write", "MultiEdit", "apply_patch"}


def _instructions():
    text = Path(__file__).with_name("cairn_instructions.txt").read_text(encoding="utf-8").strip()
    if len(text.encode()) > INSTRUCTIONS_MAX_BYTES:
        raise ValueError("Cairn instructions exceed their byte limit")
    return text


def _agent_post(origin, token, path, body):
    opener = _LOOPBACK_OPENER if origin[:5].lower() == "http:" else _OPENER
    request = urllib.request.Request(origin + path, data=json.dumps(body).encode(),
        headers={"Content-Type":"application/json", "Authorization":"Bearer " + token}, method="POST")
    with opener.open(request, timeout=2) as response:
        return json.loads(response.read(32768))


def agent_context(source, agent, config, token, origin):
    event = source.get("hook_event_name")
    if skip_capture(source, check_assistant=False):
        return {}, True
    request, _, _ = build_request(source)
    # Only delivery flags are local; never memory text, credentials or payloads.
    key = hashlib.sha256(json.dumps([origin, token, agent, request["sessionId"]]).encode()).hexdigest()
    state_path = config.parent / "hook-state.sqlite3"
    state_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(state_path, os.O_CREAT | os.O_WRONLY, 0o600)
    os.close(fd)
    text = None
    ended_work = False
    with contextlib.closing(sqlite3.connect(state_path, timeout=1)) as db:
        db.execute("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, instructed INTEGER, dirty INTEGER, worked INTEGER)")
        db.execute("BEGIN IMMEDIATE")
        row = db.execute("SELECT instructed, dirty, worked FROM sessions WHERE id=?", (key,)).fetchone()
        instructed, dirty, worked = row or (0, 0, 0)
        if event == "PostToolUse" and source.get("tool_name") in WORK_TOOLS:
            dirty, worked = 1, 1
            if not instructed:
                text, instructed = _instructions(), 1
        elif event == "Stop":
            if source.get("stop_hook_active"):
                dirty = 0
            elif dirty:
                text = "Save what is worth remembering from the new work, then finish.\n" + _instructions()
                dirty = 0
        if event == "SessionEnd":
            ended_work = bool(worked)
            db.execute("DELETE FROM sessions WHERE id=?", (key,))
        elif event in {"SessionStart", "PostToolUse", "Stop"}:
            db.execute("INSERT OR REPLACE INTO sessions VALUES (?, ?, ?, ?)", (key, instructed, dirty, worked))
        db.commit()
    if event == "SessionStart":
        text = SHORT_NOTE
        try:
            reply = _agent_post(origin, token, "/cairn-memory/session/brief", {"context":request["context"]})
            brief = reply.get("brief")
            if isinstance(brief, str) and len(brief.encode()) <= BRIEF_MAX_BYTES:
                text += "\n" + brief
        except (OSError, ValueError, http.client.HTTPException):
            sys.stderr.write("Cairn brief unavailable; use memory tools when connected.\n")
    if ended_work:
        try:
            _agent_post(origin, token, "/cairn-memory/session/end", {"sessionId":request["sessionId"]})
        except (OSError, ValueError, http.client.HTTPException):
            sys.stderr.write("Cairn session close unavailable; idle distillation remains pending.\n")
    if text is None:
        return {}, event in {"SessionStart", "SessionEnd"} or (event == "Stop" and os.environ.get("CAIRN_CAPTURE_ASSISTANT") != "1")
    if event == "Stop":
        if len(text.encode()) > END_ASK_MAX_BYTES:
            raise ValueError("Cairn end request exceeds its byte limit")
        return {"decision":"block", "reason":text}, os.environ.get("CAIRN_CAPTURE_ASSISTANT") != "1"
    return {"hookSpecificOutput":{"hookEventName":event, "additionalContext":text}}, event == "SessionStart"


def _agent_config(path):
    if not _present("CAIRN_URL") and not _present(DEVICE_ENV):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
            os.environ["CAIRN_URL"] = value["url"]
            os.environ[DEVICE_ENV] = value["token"]
        except (OSError, ValueError, KeyError, TypeError):
            raise SystemExit("cannot read valid Cairn config; run cairn setup") from None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agent", choices=["claude", "codex"])
    parser.add_argument("--config", type=Path)
    parser.add_argument("--cairn-managed", action="store_true")
    args = parser.parse_args()
    config = args.config or Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home()/".config"))) / "cairn/config.json"
    if args.agent:
        _agent_config(config)
    token = device_token()
    spool_value = _present(OUTBOX_ENV)
    bound = bool(spool_value)
    # Bad origin fails before stdin and before a lock file or a request.
    origin = _bound_origin() if bound else _unbound_base()
    raw, source = read_input()
    if args.agent:
        output, handled = agent_context(source, args.agent, config, token, origin)
        if output:
            print(scrub(json.dumps(output), token))
            sys.stdout.flush()
        if handled:
            return
    try:
        capture(source, token, raw, bound, spool_value, origin, args.agent)
    except SystemExit as error:
        if not args.agent:
            raise
        # A capture failure must not make the host discard instruction JSON already emitted.
        sys.stderr.write(str(error) + "\n")


def capture(source, token, raw, bound, spool_value, origin, agent):
    if skip_capture(source):
        return
    request, content, attachment_warning = build_request(source)
    request = prepare_body(request, token, content, raw)
    spool = None
    if bound:
        spool = Path(spool_value)
        _preflight(spool, origin, token, content, raw)
    try:
        with contextlib.redirect_stdout(io.StringIO()) if agent else contextlib.nullcontext():
            deliver(token, request, content, raw, origin, bound, attachment_warning)
    except _Retryable:
        # One POST already finished. Queue that same normalized body, or exit
        # nonzero. Do not post again and do not start a replay worker.
        _enqueue(spool, origin, token, request, content, raw)
    else:
        if bound:
            sys.stdout.flush()
            _replay_pending(spool, origin, token, request["eventId"])


if __name__ == "__main__":
    main()
