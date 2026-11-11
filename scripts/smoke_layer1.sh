#!/bin/sh
set -eu

store="$(mktemp "${TMPDIR:-/tmp}/cairn-layer1.XXXXXX")"
trap 'rm -f "$store"' EXIT
rm -f "$store"

cargo run -q -p cairn-core --bin cairn -- init "$store"
cargo run -q -p cairn-core --bin cairn -- start "$store" smoke-session cairn-demo feat/demo wt-demo abc1234 >/dev/null
cargo run -q -p cairn-core --bin cairn -- capture "$store" smoke-session evt-smoke "The auth test passed after the cookie signature fix." >/dev/null
cargo run -q -p cairn-core --bin cairn -- end "$store" smoke-session >/dev/null

before_forget="$(cargo run -q -p cairn-core --bin cairn -- context "$store" "cookie signature" 5)"
case "$before_forget" in
  *evt-smoke*) ;;
  *) echo "Layer 1 smoke check failed: captured evidence was not recalled" >&2; exit 1 ;;
esac

scoped_context="$(cargo run -q -p cairn-core --bin cairn -- context "$store" "cookie signature" 5 cairn-demo feat/demo wt-demo abc1234)"
SCOPED_CONTEXT_JSON="$scoped_context" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["SCOPED_CONTEXT_JSON"])
assert pack["results"][0]["scope"] == "worktree"
PY

resume="$(cargo run -q -p cairn-core --bin cairn -- resume "$store" 5 cairn-demo feat/demo wt-demo abc1234)"
RESUME_JSON="$resume" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["RESUME_JSON"])
assert pack["results"][0]["memory_id"] == "memory-smoke-session"
PY

why_changed="$(cargo run -q -p cairn-core --bin cairn -- why-changed "$store" "cookie signature" 5 cairn-demo feat/demo wt-demo abc1234)"
WHY_CHANGED_JSON="$why_changed" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["WHY_CHANGED_JSON"])
assert pack["results"][0]["memory_id"] == "memory-smoke-session"
assert pack["results"][0]["source_event_ids"] == ["evt-smoke"]
PY

cargo run -q -p cairn-core --bin cairn -- forget "$store" evt-smoke
after_forget="$(cargo run -q -p cairn-core --bin cairn -- search "$store" "cookie signature" 5)"
case "$after_forget" in
  *evt-smoke*) echo "Layer 1 smoke check failed: forgotten evidence was recalled" >&2; exit 1 ;;
  *) ;;
esac

lesson="$(cargo run -q -p cairn-core --bin cairn -- lesson "$store" "Keep imports idempotent." )"
LESSON_JSON="$lesson" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["LESSON_JSON"])
assert memory["kind"] == "lesson"
PY
lesson_id="$(LESSON_JSON="$lesson" python3 -c 'import json, os; print(json.loads(os.environ["LESSON_JSON"])["memory_id"])')"
corrected="$(cargo run -q -p cairn-core --bin cairn -- correct "$store" "$lesson_id" "Keep imports idempotent and preserve tombstones." )"
CORRECTED_JSON="$corrected" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["CORRECTED_JSON"])
assert memory["kind"] == "correction"
PY

echo "Layer 1/2 CLI smoke check passed"
