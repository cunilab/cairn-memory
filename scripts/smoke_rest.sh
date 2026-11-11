#!/bin/sh
set -eu

store="$(mktemp "${TMPDIR:-/tmp}/cairn-rest.XXXXXX")"
log="$(mktemp "${TMPDIR:-/tmp}/cairn-rest-log.XXXXXX")"
port="3111"
base="http://127.0.0.1:${port}/cairn-memory"
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
# Allow a clean Cargo target enough time to compile the server before polling.
for _ in $(seq 1 600); do
  if curl -fsS "$base/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if [ "$ready" -ne 1 ]; then
  printf '%s\n' "REST server did not become ready" >&2
  cat "$log" >&2
  exit 1
fi

curl -fsS -X POST "$base/session/start" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"rest-smoke","context":{"repository":"cairn-demo","branch":"main"}}' >/dev/null

curl -fsS -X POST "$base/observe" \
  -H 'Content-Type: application/json' \
  -d '{"eventId":"rest-event","sessionId":"rest-smoke","content":"The authentication test passed.","role":"agent","source":"hook"}' >/dev/null

curl -fsS -X POST "$base/session/end" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"rest-smoke"}' >/dev/null

context="$(curl -fsS -X POST "$base/context" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication test","limit":5}')"
CONTEXT_JSON="$context" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["CONTEXT_JSON"])
assert payload["results"][0]["source_event_ids"] == ["rest-event"]
PY

curl -fsS -X POST "$base/forget" \
  -H 'Content-Type: application/json' \
  -d '{"eventId":"rest-event"}' >/dev/null

after_forget="$(curl -fsS -X POST "$base/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication test","limit":5}')"
AFTER_FORGET_JSON="$after_forget" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["AFTER_FORGET_JSON"]) == []
PY

echo "REST compatibility smoke check passed"
