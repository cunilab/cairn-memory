#!/usr/bin/env python3
"""Send one AgentMemory-shaped hook payload to Cairn's observation endpoint.

The adapter keeps hook integrations transport-neutral: a hook writes one JSON
object to stdin, this script normalizes the common fields, and Cairn performs
redaction, lifecycle classification, persistence, and idempotency.
"""

import hashlib
import json
import os
import sys
import urllib.error
import urllib.request


def first(mapping, *keys):
    for key in keys:
        value = mapping.get(key)
        if value not in (None, ""):
            return value
    return None


def main():
    raw = sys.stdin.read()
    if not raw.strip():
        raise SystemExit("hook input must be one JSON object")
    source = json.loads(raw)
    nested = source.get("payload") if isinstance(source.get("payload"), dict) else {}

    session_id = first(source, "sessionId", "session_id") or first(nested, "sessionId", "session_id") or os.environ.get("CAIRN_SESSION_ID")
    if not session_id:
        raise SystemExit("hook payload requires sessionId or CAIRN_SESSION_ID")
    content = first(source, "content", "text", "message") or first(nested, "content", "text", "message")
    if not content:
        content = json.dumps(source, sort_keys=True, separators=(",", ":"))
    role = first(source, "role") or first(nested, "role") or "tool"
    source_name = first(source, "source", "hook", "type") or first(nested, "source", "type") or "hook"
    event_id = first(source, "eventId", "event_id") or first(nested, "eventId", "event_id")
    if not event_id:
        fingerprint = json.dumps({"sessionId": session_id, "content": content, "source": source_name}, sort_keys=True).encode()
        event_id = "hook-" + hashlib.sha256(fingerprint).hexdigest()[:24]

    context = first(source, "context") or first(nested, "context") or {}
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

    base_url = os.environ.get("CAIRN_URL", "http://127.0.0.1:3111").rstrip("/")
    body = json.dumps(request).encode()
    http_request = urllib.request.Request(
        base_url + "/cairn-memory/observe",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(http_request, timeout=10) as response:
            sys.stdout.write(response.read().decode())
            sys.stdout.write("\n")
    except urllib.error.HTTPError as error:
        message = error.read().decode(errors="replace")
        raise SystemExit(f"Cairn rejected hook payload ({error.code}): {message}") from error


if __name__ == "__main__":
    main()
