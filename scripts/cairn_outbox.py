#!/usr/bin/env python3
"""Bounded local outbox for authenticated Cairn sync replay."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Optional


MAX_EVENTS = 256
MAX_CONTENT_BYTES = 64 * 1024
REDACTED = "[REDACTED]"


def redact_content(content: str) -> str:
    result = content
    for marker in ("authorization: bearer ", "api_key=", "token=", "password="):
        pattern = re.compile(re.escape(marker) + r"[^\s]+", re.IGNORECASE)
        result = pattern.sub(lambda match: match.group(0)[: len(marker)] + REDACTED, result)
    return result


def load(path: Path) -> dict:
    if not path.exists():
        return {"version": 1, "events": []}
    return json.loads(path.read_text(encoding="utf-8"))


def save(path: Path, state: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(state, indent=2, sort_keys=True) + "\n"
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(payload, encoding="utf-8")
    os.replace(temporary, path)


def normalize(event: dict) -> dict:
    if not isinstance(event, dict):
        raise ValueError("outbox event must be a JSON object")
    event_id = event.get("eventId") or event.get("event_id")
    if not event_id:
        raise ValueError("outbox event requires eventId")
    normalized = dict(event)
    normalized["eventId"] = str(event_id)
    if "content" in normalized:
        content = str(normalized["content"])
        if len(content.encode()) > MAX_CONTENT_BYTES:
            raise ValueError("outbox content exceeds the 64 KiB limit")
        normalized["content"] = redact_content(content)
    return normalized


def enqueue(path: Path) -> int:
    state = load(path)
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


def replay(path: Path, base_url: str, token: str) -> int:
    state = load(path)
    remaining = []
    accepted = 0
    failed = 0
    purged = 0
    for item in state.get("events", []):
        payload = item["payload"]
        request = urllib.request.Request(
            base_url.rstrip("/") + "/cairn-memory/sync/push",
            data=json.dumps(payload).encode(),
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {token}",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                result = json.loads(response.read())
            if result.get("accepted") is True:
                accepted += 1
                continue
            raise RuntimeError("server did not acknowledge the event")
        except urllib.error.HTTPError as error:
            if error.code == 401:
                # The bearer belongs to a revoked or invalid device. Keeping
                # queued payloads would retain material that can no longer be
                # delivered under the authorized device identity.
                purged = len(state.get("events", []))
                remaining = []
                failed += 1
                break
            failed += 1
            item["attempts"] = int(item.get("attempts", 0)) + 1
            item["lastError"] = str(error)
            remaining.append(item)
        except (urllib.error.URLError, RuntimeError) as error:
            failed += 1
            item["attempts"] = int(item.get("attempts", 0)) + 1
            item["lastError"] = str(error)
            remaining.append(item)
    state["events"] = remaining
    save(path, state)
    print(json.dumps({"accepted": accepted, "failed": failed, "purged": purged, "pending": len(remaining)}))
    return 0 if failed == 0 else 1


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
            }
        )
    )
    return 0


def purge(path: Path, event_id: Optional[str]) -> int:
    state = load(path)
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
