#!/bin/sh
set -eu

cd "$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"

store="$(mktemp "${TMPDIR:-/tmp}/cairn-layer4.XXXXXX")"
trap 'rm -f "$store"' EXIT
rm -f "$store"

cairn() {
  cargo run -q -p cairn-core --bin cairn -- "$@"
}

# The memory's repository comes from start, capture, and end. capture copies
# the session context onto the observation, and end merges that observation
# context. end_session does not copy the session repository by itself. The
# commit is what makes promotion record a freshness event. One repository is
# enough; a second repository memory is not part of this flow.

cairn init "$store"

bootstrap="$(cairn account-bootstrap "$store" owner@example.test "correct horse battery staple")"
token="$(BOOTSTRAP_JSON="$bootstrap" python3 -c 'import json, os; print(json.loads(os.environ["BOOTSTRAP_JSON"])["token"])')"

project="$(cairn project-register "$store" "$token" cairn-demo "Cairn Demo" main)"
project_id="$(PROJECT_JSON="$project" python3 -c 'import json, os; print(json.loads(os.environ["PROJECT_JSON"])["project_id"])')"

listed="$(cairn project-list "$store" "$token")"
LISTED_JSON="$listed" PROJECT_ID="$project_id" python3 - <<'PY'
import json
import os

projects = json.loads(os.environ["LISTED_JSON"])
assert any(project["project_id"] == os.environ["PROJECT_ID"] for project in projects)
PY

cairn start "$store" layer4-session cairn-demo "" "" abc1234 >/dev/null
cairn capture "$store" layer4-session evt-layer4 "Promote the cache decision because the regression passed." >/dev/null
ended="$(cairn end "$store" layer4-session)"
ENDED_JSON="$ended" python3 - <<'PY'
import json
import os
import sys

memory = json.loads(os.environ["ENDED_JSON"])
repository = memory.get("context", {}).get("repository")
if repository != "cairn-demo":
    sys.exit(f"ended memory repository is {repository!r}, expected cairn-demo")
if not memory.get("context", {}).get("commit"):
    sys.exit("ended memory has no commit, so promotion would not record freshness")
PY
memory_id="$(ENDED_JSON="$ended" python3 -c 'import json, os; print(json.loads(os.environ["ENDED_JSON"])["memory_id"])')"

promoted="$(cairn promote "$store" "$token" "$project_id" "$memory_id" "Confirmed by the layer 4 CLI smoke")"
promoted_id="$(PROMOTED_JSON="$promoted" python3 -c 'import json, os; print(json.loads(os.environ["PROMOTED_JSON"])["memory_id"])')"

memories="$(cairn project-memories "$store" "$token" "$project_id")"
MEMORIES_JSON="$memories" PROMOTED_ID="$promoted_id" python3 - <<'PY'
import json
import os

memories = json.loads(os.environ["MEMORIES_JSON"])
promoted_id = os.environ["PROMOTED_ID"]
match = [memory for memory in memories if memory["memory_id"] == promoted_id]
assert len(match) == 1
assert match[0]["scope"] == "project"
assert len(memories) == 1
PY

freshness="$(cairn project-freshness "$store" "$token" "$project_id")"
FRESHNESS_JSON="$freshness" python3 - <<'PY'
import json
import os

events = json.loads(os.environ["FRESHNESS_JSON"])
assert any(event["action"] == "promotion" for event in events)
PY

corrected="$(cairn correct "$store" "$promoted_id" "The cache decision stays project-scoped after correction.")"
correction_id="$(CORRECTED_JSON="$corrected" python3 -c 'import json, os; print(json.loads(os.environ["CORRECTED_JSON"])["memory_id"])')"

after="$(cairn project-memories "$store" "$token" "$project_id")"
AFTER_JSON="$after" PROMOTED_ID="$promoted_id" CORRECTION_ID="$correction_id" python3 - <<'PY'
import json
import os

memories = json.loads(os.environ["AFTER_JSON"])
promoted_id = os.environ["PROMOTED_ID"]
correction_id = os.environ["CORRECTION_ID"]
assert all(memory["memory_id"] != promoted_id for memory in memories)
match = [memory for memory in memories if memory["memory_id"] == correction_id]
assert len(match) == 1
assert match[0]["supersedes"] == promoted_id
assert match[0]["scope"] == "project"
PY

echo "Layer 4 CLI smoke check passed"
