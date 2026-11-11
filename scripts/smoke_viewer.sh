#!/bin/sh
set -eu

store="$(mktemp "${TMPDIR:-/tmp}/cairn-viewer.XXXXXX")"
log="$(mktemp "${TMPDIR:-/tmp}/cairn-viewer-log.XXXXXX")"
port="3112"
pid=""

cleanup() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -f "$store" "$log"
}
trap cleanup EXIT INT TERM
rm -f "$store"

cargo run -q -p cairn-server -- --store "$store" --bind "127.0.0.1:${port}" >"$log" 2>&1 &
pid=$!

ready=0
for _ in $(seq 1 600); do
  if curl -fsS "http://127.0.0.1:${port}/cairn-memory/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if [ "$ready" -ne 1 ]; then
  printf '%s\n' "viewer server did not become ready" >&2
  cat "$log" >&2
  exit 1
fi

page="$(curl -fsS "http://127.0.0.1:${port}/viewer")"
PAGE_HTML="$page" python3 - <<'PY'
import os

page = os.environ["PAGE_HTML"]
for marker in ("Cairn / memory field", "/cairn-memory/export", "Forget this", "Invite member", "Share decision", "Revoke share", "Create team", "Add member", "Remove member", "Delete memory", "/shared", "Current decisions", "Handoffs", "Stale memories", "Conflicts", "Revocations", "Content-Type"):
    assert marker in page, marker
loader = page.split("async function loadProjectMemories", 1)[1]
member_at = loader.index("if (member)")
shared_at = loader.index("/shared")
owner_at = loader.index("/memories")
assert member_at < shared_at < owner_at
assert "/memories" not in loader[member_at:shared_at]
PY

diagnose="$(curl -fsS "http://127.0.0.1:${port}/cairn-memory/diagnose")"
DIAGNOSE_JSON="$diagnose" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["DIAGNOSE_JSON"])
assert payload["temporary_observations"] == 0
assert payload["durable_memories"] == 0
PY

echo "Viewer smoke check passed"
