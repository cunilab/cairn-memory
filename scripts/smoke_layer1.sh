#!/bin/sh
set -eu

. "$(dirname "$0")/smoke_agent_lib.sh"

start_server 3131
create_agent layer1 layer1@example.test bootstrap
use_agent layer1

cairn start smoke-session cairn-demo feat/demo wt-demo abc1234 >/dev/null
cairn capture smoke-session evt-smoke "The auth test passed after the cookie signature fix." >/dev/null
cairn end smoke-session >/dev/null

before_forget="$(cairn context "cookie signature" 5)"
case "$before_forget" in
  *evt-smoke*) ;;
  *) echo "Layer 1 smoke check failed: captured evidence was not recalled" >&2; exit 1 ;;
esac

scoped_context="$(cairn context "cookie signature" 5 cairn-demo feat/demo wt-demo abc1234)"
SCOPED_CONTEXT_JSON="$scoped_context" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["SCOPED_CONTEXT_JSON"])
assert pack["results"][0]["scope"] == "worktree"
PY

resume="$(cairn resume 5 cairn-demo feat/demo wt-demo abc1234)"
RESUME_JSON="$resume" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["RESUME_JSON"])
assert pack["results"][0]["memory_id"].endswith("-smoke-session")
PY

why_changed="$(cairn why-changed "cookie signature" 5 cairn-demo feat/demo wt-demo abc1234)"
WHY_CHANGED_JSON="$why_changed" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["WHY_CHANGED_JSON"])
assert pack["results"][0]["memory_id"].endswith("-smoke-session")
assert pack["results"][0]["source_event_ids"] == ["evt-smoke"]
PY

cairn forget evt-smoke
after_forget="$(cairn search "cookie signature" 5)"
case "$after_forget" in
  *evt-smoke*) echo "Layer 1 smoke check failed: forgotten evidence was recalled" >&2; exit 1 ;;
  *) ;;
esac

lesson="$(cairn lesson "Keep imports idempotent." )"
LESSON_JSON="$lesson" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["LESSON_JSON"])
assert memory["kind"] == "lesson"
PY
lesson_id="$(LESSON_JSON="$lesson" python3 -c 'import json, os; print(json.loads(os.environ["LESSON_JSON"])["memory_id"])')"
corrected="$(cairn correct "$lesson_id" "Keep imports idempotent and preserve tombstones." )"
CORRECTED_JSON="$corrected" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["CORRECTED_JSON"])
assert memory["kind"] == "correction"
PY

echo "Layer 1/2 CLI smoke check passed"
