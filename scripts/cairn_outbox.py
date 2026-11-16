#!/usr/bin/env python3
"""Bounded local outbox for authenticated Cairn sync replay.

Version 1 enqueue, replay, and purge keep the unbound queue. Version 2 is
created only by enqueue_bound when the file is missing. The hook appends to
that file only for a retryable device delivery when CAIRN_OUTBOX is set, and
only with the nonblocking lock. Replay of a version 2 file checks the origin,
route, and device fingerprint before it opens a socket. There is no rebind
command. Replay does not read CAIRN_DEVICE_TOKEN.
"""

from __future__ import annotations

import argparse
import errno
import hashlib
import hmac
import http.client
import ipaddress
import json
import os
import re
import stat
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from contextlib import contextmanager
from pathlib import Path
from typing import Optional
from urllib.parse import urlsplit

try:
    import fcntl
except ImportError:
    fcntl = None  # type: ignore[assignment]


# Keep VERSION equal to [workspace.package] version in Cargo.toml (smoke_outbox.py checks it).
VERSION = "0.1.4"
# urllib's default agent is blocked by Cloudflare (error 1010) in front of a deployed server.
USER_AGENT = "cairn-hook/" + VERSION
MAX_EVENTS = 256
MAX_CONTENT_BYTES = 64 * 1024
MAX_BOUNDED_ATTEMPTS = 3
REDACTED = "[REDACTED]"
_ASCII_LOWER = str.maketrans(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "abcdefghijklmnopqrstuvwxyz",
)
# Exact labels from crates/core PRIVATE_KEY_LABELS.
_PRIVATE_KEY_LABELS = (
    "encrypted private key",
    "openssh private key",
    "rsa private key",
    "ec private key",
    "private key",
)


def _ascii_lower(text: str) -> str:
    return text.translate(_ASCII_LOWER)


def redact_content(content: str) -> str:
    result = content
    for marker in ("authorization: bearer ", "api_key=", "token=", "password="):
        pattern = re.compile(re.escape(marker) + r"[^\s]*", re.IGNORECASE)
        result = pattern.sub(lambda match: match.group(0)[: len(marker)] + REDACTED, result)
    result = _redact_prefixed_secret(result, "sk-")
    return _redact_private_key_blocks(result)


# Mirrors crates/core find_token_prefix: match only where a token starts, keeping words such as "disk-full".
def _find_token_prefix(lower: str, prefix: str, start: int) -> int:
    while True:
        found = lower.find(prefix, start)
        if found <= 0 or not lower[found - 1].isalnum():
            return found
        start = found + len(prefix)


def _redact_prefixed_secret(content: str, prefix: str) -> str:
    lower = _ascii_lower(content)
    prefix = _ascii_lower(prefix)
    output: list[str] = []
    cursor = 0
    while True:
        start = _find_token_prefix(lower, prefix, cursor)
        if start < 0:
            break
        value_end = len(content)
        for index in range(start, len(content)):
            if content[index].isspace():
                value_end = index
                break
        output.append(content[cursor:start])
        output.append(REDACTED)
        cursor = value_end
    output.append(content[cursor:])
    return "".join(output)


def _known_private_key_label(label: str) -> Optional[str]:
    for candidate in _PRIVATE_KEY_LABELS:
        if candidate == label:
            return candidate
    return None


def _private_key_header(line: str) -> Optional[tuple[int, str]]:
    lower = _ascii_lower(line)
    search_from = 0
    begin = "-----begin "
    while True:
        start = lower.find(begin, search_from)
        if start < 0:
            return None
        after = start + len(begin)
        label_end = lower.find("-----", after)
        if label_end < 0:
            return None
        label = _known_private_key_label(lower[after:label_end])
        if label is not None:
            return start, label
        search_from = label_end + len("-----")


def _is_pem_continuation_line(line: str) -> bool:
    trimmed = line.rstrip("\r\n").strip()
    if not trimmed:
        return True
    lower = _ascii_lower(trimmed)
    if lower.startswith("proc-type:") or lower.startswith("dek-info:"):
        return True
    return all(
        character.isascii() and (character.isalnum() or character in "+/=")
        for character in trimmed
    )


def _truncated_footer_resume(line_lower: str, label: str) -> Optional[int]:
    """Offset after optional PEM base64 glued to a truncated -----END label."""
    token = f"-----end {label}"
    index = 0
    alphabet = "abcdefghijklmnopqrstuvwxyz0123456789+/="
    while index < len(line_lower) and line_lower[index] in alphabet:
        index += 1
    if not line_lower.startswith(token, index):
        return None
    after = index + len(token)
    if line_lower.startswith("-----", after):
        return None
    return after


def _redact_private_key_blocks(content: str) -> str:
    # A recognized header with no END drops that line and following base64 or
    # OpenSSL metadata. Base64 glued to a truncated -----END label drops that
    # prefix and the footer token; text after the token stays.
    output: list[str] = []
    cursor = 0
    length = len(content)
    while cursor < length:
        newline = content.find("\n", cursor)
        next_newline = length if newline < 0 else newline + 1
        line = content[cursor:next_newline]
        header = _private_key_header(line)
        if header is None:
            output.append(line)
            cursor = next_newline
            continue
        header_at, label = header
        output.append(line[:header_at])
        output.append(REDACTED)
        end_marker = f"-----end {label}-----"
        end_at = _ascii_lower(line).find(end_marker, header_at)
        if end_at >= 0:
            cursor += end_at + len(end_marker)
            continue
        if line.endswith("\n"):
            output.append("\n")
        cursor = next_newline
        while cursor < length:
            body_newline = content.find("\n", cursor)
            body_end = length if body_newline < 0 else body_newline + 1
            body = content[cursor:body_end]
            body_lower = _ascii_lower(body)
            footer = body_lower.find(end_marker)
            if footer >= 0:
                cursor += footer + len(end_marker)
                break
            truncated = _truncated_footer_resume(body_lower, label)
            if truncated is not None:
                cursor += truncated
                break
            if not _is_pem_continuation_line(body):
                break
            cursor = body_end
    return "".join(output)


# Fixed text only. It must not name a platform we do not lock, or any payload.
_LOCK_MESSAGE = (
    "outbox locking requires Unix fcntl.flock; refusing to update the queue unlocked"
)
# LOCK_NB only. Callers must not wait, sleep, or write without the lock.
_BUSY_MESSAGE = "outbox queue is busy"
_held_locks = threading.local()


def _lock_path(path: Path) -> Path:
    return Path(os.path.abspath(path) + ".lock")


def _held_lock_ids() -> set[tuple[int, int]]:
    held = getattr(_held_locks, "ids", None)
    if held is None:
        held = set()
        _held_locks.ids = held
    return held


@contextmanager
def _queue_lock(path: Path, *, nonblocking: bool = False):
    # Same-thread reentry is the lock inode from fstat, not the path spelling.
    # The extra fd is closed and the outer owner keeps the flock. This set does
    # not skip another thread; flock on that inode still excludes it.
    # nonblocking is the hook's preflight and append. CLI callers keep waiting.
    # A busy lock is not an unlocked write.
    if fcntl is None or not hasattr(fcntl, "flock"):
        raise RuntimeError(_LOCK_MESSAGE)
    lock_path = _lock_path(path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR | os.O_CLOEXEC, 0o600)
    try:
        info = os.fstat(fd)
        identity = (info.st_dev, info.st_ino)
        held = _held_lock_ids()
        if identity in held:
            os.close(fd)
            fd = -1
            yield
            return
        flags = fcntl.LOCK_EX
        if nonblocking:
            flags |= fcntl.LOCK_NB
        try:
            fcntl.flock(fd, flags)
        except OSError as error:
            if nonblocking and error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                raise RuntimeError(_BUSY_MESSAGE) from error
            raise RuntimeError(_LOCK_MESSAGE) from error
        held.add(identity)
        try:
            yield
        finally:
            held.discard(identity)
            fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        if fd >= 0:
            os.close(fd)


def load(path: Path) -> dict:
    if not path.exists():
        return {"version": 1, "events": []}
    return json.loads(path.read_text(encoding="utf-8"))


def _load_state(path: Path) -> dict:
    # A corrupt file must not echo its bytes into the error text.
    try:
        state = load(path)
    except json.JSONDecodeError:
        raise ValueError(_METADATA_MESSAGE) from None
    if not isinstance(state, dict):
        raise ValueError(_METADATA_MESSAGE)
    return state


def _write_all(fd: int, payload: bytes) -> None:
    view = memoryview(payload)
    while view:
        written = os.write(fd, view)
        if written <= 0:
            raise OSError("outbox temp write failed")
        view = view[written:]


def save(path: Path, state: dict) -> None:
    # Unique name: a shared ".tmp" is replaced by a concurrent writer.
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = (json.dumps(state, indent=2, sort_keys=True) + "\n").encode()
    fd, name = tempfile.mkstemp(
        prefix="." + path.name + ".",
        suffix=".tmp",
        dir=path.parent,
    )
    closed = False
    replaced = False
    try:
        if path.exists():
            os.fchmod(fd, stat.S_IMODE(path.stat().st_mode))
        _write_all(fd, payload)
        os.fsync(fd)
        os.close(fd)
        closed = True
        os.replace(name, path)
        replaced = True
    finally:
        if not closed:
            os.close(fd)
        if not replaced:
            try:
                os.unlink(name)
            except FileNotFoundError:
                pass


# Exact WorkContext list names. No aliases. Missing keys stay missing.
_CONTEXT_LISTS = (
    "files",
    "symbols",
    "commands",
    "tests",
    "services",
    "handoffs",
    "runbooks",
)


def _redact_context_lists(context: dict) -> dict:
    copied = dict(context)
    for key in _CONTEXT_LISTS:
        if key not in copied:
            continue
        values = copied[key]
        if not isinstance(values, list) or not all(isinstance(item, str) for item in values):
            raise ValueError(f"outbox context {key} must be a list of strings")
        copied[key] = [redact_content(item) for item in values]
    return copied


def normalize(event: dict) -> dict:
    if not isinstance(event, dict):
        raise ValueError("outbox event must be a JSON object")
    event_id = event.get("eventId") or event.get("event_id")
    if not event_id:
        raise ValueError("outbox event requires eventId")
    normalized = dict(event)
    normalized["eventId"] = str(event_id)
    if "context" in normalized and normalized["context"] is not None:
        context = normalized["context"]
        if not isinstance(context, dict):
            raise ValueError("outbox context must be a JSON object")
        normalized["context"] = _redact_context_lists(context)
    if "content" in normalized:
        content = str(normalized["content"])
        if len(content.encode()) > MAX_CONTENT_BYTES:
            raise ValueError("outbox content exceeds the 64 KiB limit")
        normalized["content"] = redact_content(content)
    return normalized


_ORIGIN_MESSAGE = (
    "outbox origin must be an http(s) server base URL with no userinfo, query, or fragment"
)
_TRANSPORT_MESSAGE = "plain http is allowed only for loopback hosts; use https"
_ROUTE = "sync/push"
_FINGERPRINT_PREFIX = "cairn-outbox-v2\0"
_ACCOUNT_TOKEN_MESSAGE = "outbox device token must not be the configured account token"
_TOKEN_MESSAGE = "outbox device token is required"
_VERSION_MESSAGE = "outbox queue version is not supported"
_METADATA_MESSAGE = "outbox queue metadata is invalid"


def _is_loopback(host: str) -> bool:
    # Same set as crates/client normalize_url. A mapped ::ffff:127.0.0.1 is not
    # loopback there, so only the plain ::1 counts for IPv6.
    if host.lower() == "localhost":
        return True
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        return False
    return address == ipaddress.IPv6Address("::1") or (
        address.version == 4 and address.is_loopback
    )


def require_secure_url(base: str) -> None:
    # For a URL the token is sent to without the strict origin form (hook
    # without a queue, version 1 replay): a path stays allowed, plain http does not.
    try:
        parts = urlsplit(base)
        host = parts.hostname
        parts.port  # raises on a malformed port
    except ValueError:
        raise ValueError(_ORIGIN_MESSAGE) from None
    scheme = parts.scheme.lower()
    if (
        scheme not in ("http", "https")
        or not host
        or parts.username is not None
        or parts.password is not None
        or parts.query
        or parts.fragment
    ):
        raise ValueError(_ORIGIN_MESSAGE)
    if scheme == "http" and not _is_loopback(host):
        raise ValueError(_TRANSPORT_MESSAGE)


def normalize_origin(base: str) -> str:
    # CAIRN_URL is the server base the hook posts to after stripping trailing
    # slashes: origin plus an optional path prefix (a reverse proxy mount such
    # as /agent). The prefix must not already name the API route, repeat a slash,
    # or hold dot segments or percent escapes, so the stored base maps to one
    # request URL.
    if (
        not isinstance(base, str)
        or base == ""
        or any(ord(character) <= 32 or ord(character) == 127 for character in base)
    ):
        raise ValueError(_ORIGIN_MESSAGE)
    try:
        parts = urlsplit(base.rstrip("/"))
        port = parts.port
    except ValueError:
        raise ValueError(_ORIGIN_MESSAGE) from None
    scheme = parts.scheme.lower()
    if scheme not in ("http", "https"):
        raise ValueError(_ORIGIN_MESSAGE)
    if parts.username is not None or parts.password is not None:
        raise ValueError(_ORIGIN_MESSAGE)
    if parts.query or parts.fragment:
        raise ValueError(_ORIGIN_MESSAGE)
    segments = parts.path.split("/")[1:]
    if "%" in parts.path or any(
        segment in ("", ".", "..", "cairn-memory") for segment in segments
    ):
        raise ValueError(_ORIGIN_MESSAGE)
    host = parts.hostname
    # Request unquotes a % in the host, so %2e or %3a would post somewhere
    # other than the stored origin. Do not decode it. A zone id uses % too.
    if not host or "%" in host:
        raise ValueError(_ORIGIN_MESSAGE)
    if scheme == "http" and not _is_loopback(host):
        raise ValueError(_TRANSPORT_MESSAGE)
    if ":" in host:
        host = "[" + host + "]"
    default_port = 80 if scheme == "http" else 443
    netloc = host if port is None or port == default_port else f"{host}:{port}"
    return f"{scheme}://{netloc}{parts.path}"


def device_fingerprint(token: str) -> str:
    if not isinstance(token, str) or token == "":
        raise ValueError(_TOKEN_MESSAGE)
    try:
        material = (_FINGERPRINT_PREFIX + token).encode("utf-8")
    except UnicodeEncodeError:
        raise ValueError(_TOKEN_MESSAGE) from None
    return hashlib.sha256(material).hexdigest()


def _reject_account_token(token: str) -> None:
    # Replay does not read this value as a bearer. Equal tokens are refused
    # before a socket. Version 1 replay does not call this.
    if not isinstance(token, str) or token == "":
        raise ValueError(_TOKEN_MESSAGE)
    account = os.environ.get("CAIRN_ACCOUNT_TOKEN")
    if account and token == account:
        raise ValueError(_ACCOUNT_TOKEN_MESSAGE)


def _fingerprint_matches(stored: str, token: str) -> bool:
    actual = device_fingerprint(token)
    if len(stored) != len(actual):
        return False
    return hmac.compare_digest(stored, actual)


def _version2_metadata(state: dict) -> bool:
    return (
        isinstance(state.get("route"), str)
        and isinstance(state.get("origin"), str)
        and isinstance(state.get("deviceFingerprint"), str)
        and isinstance(state.get("events"), list)
    )


def _require_bound_file(state: dict, origin: str, token: str) -> None:
    version = state.get("version", 1)
    if version != 2:
        raise ValueError("outbox file is not a version 2 queue")
    if not _version2_metadata(state):
        raise ValueError(_METADATA_MESSAGE)
    if state.get("route") != _ROUTE:
        raise ValueError("outbox route does not match this queue")
    try:
        stored = normalize_origin(state["origin"])
    except ValueError:
        raise ValueError(_METADATA_MESSAGE) from None
    if stored != origin:
        raise ValueError("outbox origin does not match this queue")
    fingerprint = state["deviceFingerprint"]
    if not isinstance(fingerprint, str) or not _fingerprint_matches(fingerprint, token):
        raise ValueError("outbox device token does not match this queue")


def _version2_post_origin(state: dict, base_url: str, token: str) -> str:
    # Binding is decided before any socket. A mismatch must not save or purge.
    _reject_account_token(token)
    if not _version2_metadata(state):
        raise ValueError(_METADATA_MESSAGE)
    try:
        supplied = normalize_origin(base_url)
    except ValueError:
        raise ValueError(_ORIGIN_MESSAGE) from None
    _require_bound_file(state, supplied, token)
    return supplied


def enqueue(path: Path) -> int:
    with _queue_lock(path):
        state = load(path)
        if state.get("version", 1) != 1:
            raise ValueError("outbox file is not a version 1 queue")
        event = normalize(json.load(sys.stdin))
        events = state.setdefault("events", [])
        if any(item.get("payload", {}).get("eventId") == event["eventId"] for item in events):
            print(json.dumps({"queued": False, "duplicate": True, "eventId": event["eventId"]}))
            return 0
        if len(events) >= MAX_EVENTS:
            raise ValueError(f"outbox is full at {MAX_EVENTS} events")
        events.append({"payload": event, "attempts": 0, "lastError": None})
        save(path, state)
        print(json.dumps({"queued": True, "duplicate": False, "eventId": event["eventId"]}))
        return 0


def preflight_bound(path: Path, origin: str, token: str, *, nonblocking: bool = False) -> None:
    # A missing queue is not created. The sibling lock file may be. This does
    # not replace the check inside enqueue_bound.
    _reject_account_token(token)
    canonical = normalize_origin(origin)
    with _queue_lock(path, nonblocking=nonblocking):
        if not path.exists():
            return
        _require_bound_file(_load_state(path), canonical, token)


def enqueue_bound(
    path: Path,
    event: dict,
    origin: str,
    token: str,
    *,
    nonblocking: bool = False,
    report: bool = True,
) -> int:
    # One file is one device token and one origin. A new token does not rebind.
    # event is the explicit payload. The hook does not use stdin enqueue.
    _reject_account_token(token)
    canonical = normalize_origin(origin)
    normalized = normalize(event)
    with _queue_lock(path, nonblocking=nonblocking):
        if path.exists():
            state = _load_state(path)
            _require_bound_file(state, canonical, token)
            events = state["events"]
        else:
            state = {
                "version": 2,
                "route": _ROUTE,
                "origin": canonical,
                "deviceFingerprint": device_fingerprint(token),
                "events": [],
            }
            events = state["events"]
        duplicate = any(
            item.get("payload", {}).get("eventId") == normalized["eventId"] for item in events
        )
        if duplicate:
            if report:
                print(json.dumps({"queued": False, "duplicate": True, "eventId": normalized["eventId"]}))
            return 0
        if len(events) >= MAX_EVENTS:
            raise ValueError(f"outbox is full at {MAX_EVENTS} events")
        events.append({"payload": normalized, "attempts": 0, "lastError": None})
        save(path, state)
        if report:
            print(json.dumps({"queued": True, "duplicate": False, "eventId": normalized["eventId"]}))
        return 0


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def http_error_302(self, req, fp, code, msg, headers):
        # Parent http_error_302 interpolates a non-http Location into the
        # error before redirect_request, and reading the body can raise
        # IncompleteRead or block. Close with a fixed message instead.
        fp.close()
        raise urllib.error.HTTPError(req.full_url, code, "redirect blocked", None, None)

    http_error_301 = http_error_303 = http_error_307 = http_error_308 = http_error_302


_OPENER = urllib.request.build_opener(_NoRedirect)
# Plain http only reaches loopback, so the token never goes through HTTP_PROXY.
_LOOPBACK_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect)


def _retryable_status(code: int) -> bool:
    return code == 408 or code == 429 or code >= 500


def _replay_loaded(
    path: Path,
    state: dict,
    base_url: str,
    token: str,
    *,
    limit: Optional[int] = None,
    deadline: Optional[float] = None,
    bounded: bool = False,
    skip_event_id: Optional[str] = None,
    report: bool = True,
) -> int:
    # The keyword arguments are the hook's bounded pass. Events not attempted
    # stay pending and unchanged. bounded never purges (a 401 stops the pass),
    # stops at the first outage, and skips events that already failed
    # MAX_BOUNDED_ATTEMPTS times so they cannot starve newer ones. The CLI
    # passes none of them. A 409 means the server holds different content under
    # that event id, so resending cannot succeed: the event leaves the queue,
    # the CLI reports it as rejected, and the queue metadata counts it.
    remaining = []
    rejected = []
    accepted = 0
    failed = 0
    purged = 0
    attempted = 0
    stop = False
    for item in state.get("events", []):
        payload = item["payload"]
        if (
            stop
            or payload.get("eventId") == skip_event_id
            or (bounded and int(item.get("attempts", 0)) >= MAX_BOUNDED_ATTEMPTS)
        ):
            remaining.append(item)
            continue
        timeout = 10
        if deadline is not None:
            # One request never outlives the budget that is left.
            timeout = min(timeout, deadline - time.monotonic())
        if (limit is not None and attempted >= limit) or timeout <= 0:
            stop = True
            remaining.append(item)
            continue
        attempted += 1
        request = urllib.request.Request(
            base_url + "/cairn-memory/sync/push",
            data=json.dumps(payload).encode(),
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {token}",
                "User-Agent": USER_AGENT,
            },
            method="POST",
        )
        try:
            opener = _LOOPBACK_OPENER if base_url[:5].lower() == "http:" else _OPENER
            with opener.open(request, timeout=timeout) as response:
                result = json.loads(response.read())
            if isinstance(result, dict) and result.get("accepted") is True:
                accepted += 1
                continue
            raise RuntimeError("server did not acknowledge the event")
        except urllib.error.HTTPError as error:
            if error.code == 401 and not bounded:
                # The bearer belongs to a revoked or invalid device. Keeping
                # queued payloads would retain material that can no longer be
                # delivered under the authorized device identity.
                purged = len(state.get("events", []))
                remaining = []
                failed += 1
                break
            if error.code == 409:
                rejected.append({"eventId": payload.get("eventId"), "reason": str(error)})
                continue
            failed += 1
            item["attempts"] = int(item.get("attempts", 0)) + 1
            item["lastError"] = str(error)
            remaining.append(item)
            stop = bounded and (error.code == 401 or _retryable_status(error.code))
        # A bad ack or a mid-read failure must keep this event and the loop's progress.
        except (OSError, ValueError, http.client.HTTPException, RuntimeError) as error:
            failed += 1
            item["attempts"] = int(item.get("attempts", 0)) + 1
            item["lastError"] = str(error)
            remaining.append(item)
            stop = bounded
    state["events"] = remaining
    if rejected:
        state["rejectedCount"] = int(state.get("rejectedCount", 0)) + len(rejected)
        state["lastRejected"] = rejected[-1]
    save(path, state)
    if report:
        print(
            json.dumps(
                {
                    "accepted": accepted,
                    "failed": failed,
                    "purged": purged,
                    "pending": len(remaining),
                    "rejected": rejected,
                }
            )
        )
    return 0 if failed == 0 and not rejected else 1


def replay(path: Path, base_url: str, token: str) -> int:
    # Held across the socket calls (timeout=10 each). Releasing before save
    # would let an enqueue land and then be overwritten. The hook uses LOCK_NB
    # on this same lock and does not wait. This does not add a worker.
    # --token is the only credential. CAIRN_DEVICE_TOKEN is not read.
    # A missing queue is not created, and neither is its lock file.
    if not path.exists():
        print(json.dumps({"accepted": 0, "failed": 0, "purged": 0, "pending": 0, "note": "nothing to replay"}))
        return 0
    with _queue_lock(path):
        state = load(path)
        version = state.get("version", 1)
        if version == 1:
            require_secure_url(base_url)
            return _replay_loaded(path, state, base_url.rstrip("/"), token)
        if version != 2:
            raise ValueError(_VERSION_MESSAGE)
        origin = _version2_post_origin(state, base_url, token)
        return _replay_loaded(path, state, origin, token)


def replay_bounded(
    path: Path,
    origin: str,
    token: str,
    *,
    limit: int,
    budget: float,
    skip_event_id: Optional[str] = None,
) -> None:
    # The hook's opportunistic pass: LOCK_NB, a version 2 file bound to this
    # origin and token, at most limit events inside budget seconds, no purge,
    # and the first outage or 401 stops it. Raises RuntimeError when the lock is busy.
    # Prints nothing. The caller decides what a failure means.
    if not path.exists():
        return
    with _queue_lock(path, nonblocking=True):
        state = _load_state(path)
        if state.get("version", 1) != 2:
            raise ValueError(_VERSION_MESSAGE)
        supplied = _version2_post_origin(state, origin, token)
        _replay_loaded(
            path,
            state,
            supplied,
            token,
            limit=limit,
            deadline=time.monotonic() + budget,
            bounded=True,
            skip_event_id=skip_event_id,
            report=False,
        )


def status(path: Path) -> int:
    state = load(path)
    events = state.get("events", [])
    print(
        json.dumps(
            {
                "version": state.get("version", 1),
                "pending": len(events),
                "attempts": sum(int(item.get("attempts", 0)) for item in events),
                "hasErrors": any(item.get("lastError") for item in events),
                "rejectedCount": int(state.get("rejectedCount", 0)),
                "lastRejected": state.get("lastRejected"),
            }
        )
    )
    return 0


def purge(path: Path, event_id: Optional[str]) -> int:
    # Like replay, a missing queue is not created, and neither is its lock file.
    if not path.exists():
        print(json.dumps({"purged": 0, "pending": 0}))
        return 0
    with _queue_lock(path):
        state = load(path)
        version = state.get("version", 1)
        if version not in (1, 2):
            raise ValueError(_VERSION_MESSAGE)
        if version == 2 and not _version2_metadata(state):
            raise ValueError(_METADATA_MESSAGE)
        events = state.get("events", [])
        if event_id is None:
            removed = len(events)
            state["events"] = []
        else:
            state["events"] = [
                item for item in events if item.get("payload", {}).get("eventId") != event_id
            ]
            removed = len(events) - len(state["events"])
        save(path, state)
        print(json.dumps({"purged": removed, "pending": len(state["events"])}))
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    enqueue_parser = subparsers.add_parser("enqueue")
    enqueue_parser.add_argument("outbox", type=Path)

    replay_parser = subparsers.add_parser("replay")
    replay_parser.add_argument("outbox", type=Path)
    replay_parser.add_argument("--url", default="http://127.0.0.1:3111")
    replay_parser.add_argument("--token", required=True)

    status_parser = subparsers.add_parser("status")
    status_parser.add_argument("outbox", type=Path)

    purge_parser = subparsers.add_parser("purge")
    purge_parser.add_argument("outbox", type=Path)
    purge_parser.add_argument("--event-id")

    args = parser.parse_args()
    if args.command == "enqueue":
        return enqueue(args.outbox)
    if args.command == "replay":
        return replay(args.outbox, args.url, args.token)
    if args.command == "purge":
        return purge(args.outbox, args.event_id)
    return status(args.outbox)


if __name__ == "__main__":
    raise SystemExit(main())
