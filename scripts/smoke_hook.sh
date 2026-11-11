#!/bin/sh
set -eu

store="$(mktemp "${TMPDIR:-/tmp}/cairn-hook.XXXXXX")"
log="$(mktemp "${TMPDIR:-/tmp}/cairn-hook-log.XXXXXX")"
port="3114"
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
  cat "$log" >&2
  exit 1
fi

curl -fsS -X POST "http://127.0.0.1:${port}/cairn-memory/session/start" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"hook-smoke-session"}' >/dev/null

hook_response="$(CAIRN_URL="http://127.0.0.1:${port}" python3 scripts/cairn_hook.py <<'JSON'
{"eventId":"hook-smoke-event","sessionId":"hook-smoke-session","content":"The portable hook captured this decision.","role":"tool","source":"PostToolUse","importance":"high","context":{"repository":"cairn-demo","branch":"main"}}
JSON
)"
HOOK_JSON="$hook_response" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["HOOK_JSON"])
assert payload["accepted"] is True
assert payload["observation"]["event_id"] == "hook-smoke-event"
PY

curl -fsS -X POST "http://127.0.0.1:${port}/cairn-memory/session/end" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"hook-smoke-session"}' >/dev/null

results="$(curl -fsS -X POST "http://127.0.0.1:${port}/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"portable hook decision","limit":5}')"
RESULTS_JSON="$results" python3 - <<'PY'
import json
import os

assert len(json.loads(os.environ["RESULTS_JSON"])) == 1
PY

echo "Hook adapter smoke check passed"
