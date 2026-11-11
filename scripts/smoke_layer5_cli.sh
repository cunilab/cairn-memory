#!/bin/sh
set -eu

cd "$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"

store="$(mktemp "${TMPDIR:-/tmp}/cairn-layer5.XXXXXX")"
trap 'rm -f "$store"' EXIT
rm -f "$store"

cairn() {
  cargo run -q -p cairn-core --bin cairn -- "$@"
}

# The memory's repository comes from start, capture, and end. capture copies
# the session context onto the observation, and end merges that observation
# context. end_session does not copy the session repository by itself.

cairn init "$store"

bootstrap="$(cairn account-bootstrap "$store" owner@example.test "correct horse battery staple")"
owner_token="$(BOOTSTRAP_JSON="$bootstrap" python3 -c 'import json, os; print(json.loads(os.environ["BOOTSTRAP_JSON"])["token"])')"

registered="$(cairn account-register "$store" member@example.test "correct horse battery staple")"
member_token="$(REGISTERED_JSON="$registered" python3 -c 'import json, os; print(json.loads(os.environ["REGISTERED_JSON"])["token"])')"

project="$(cairn project-register "$store" "$owner_token" cairn-demo "Cairn Demo" main)"
project_id="$(PROJECT_JSON="$project" python3 -c 'import json, os; print(json.loads(os.environ["PROJECT_JSON"])["project_id"])')"

cairn start "$store" private-session >/dev/null
cairn capture "$store" private-session evt-private "private observation stays on the owner account" >/dev/null
private_ended="$(cairn end "$store" private-session)"
private_id="$(PRIVATE_JSON="$private_ended" python3 -c 'import json, os; print(json.loads(os.environ["PRIVATE_JSON"])["memory_id"])')"

cairn start "$store" layer5-session cairn-demo "" "" abc1234 >/dev/null
cairn capture "$store" layer5-session evt-layer5 "repository cache decision is ready for the project" >/dev/null
ended="$(cairn end "$store" layer5-session)"
memory_id="$(ENDED_JSON="$ended" python3 -c 'import json, os; print(json.loads(os.environ["ENDED_JSON"])["memory_id"])')"

promoted="$(cairn promote "$store" "$owner_token" "$project_id" "$memory_id" "Share the promoted repository decision")"
promoted_id="$(PROMOTED_JSON="$promoted" python3 -c 'import json, os; print(json.loads(os.environ["PROMOTED_JSON"])["memory_id"])')"

cairn project-invite "$store" "$owner_token" "$project_id" member@example.test >/dev/null
cairn project-share "$store" "$owner_token" "$project_id" "$promoted_id" member@example.test >/dev/null

shared="$(cairn project-shared "$store" "$member_token" "$project_id")"
SHARED_JSON="$shared" PROMOTED_ID="$promoted_id" PRIVATE_ID="$private_id" python3 - <<'PY'
import json
import os

memories = json.loads(os.environ["SHARED_JSON"])
assert [memory["memory_id"] for memory in memories] == [os.environ["PROMOTED_ID"]]
assert all(memory["memory_id"] != os.environ["PRIVATE_ID"] for memory in memories)
PY

search="$(cairn search "$store" "private observation" 5 cairn-demo)"
SEARCH_JSON="$search" PRIVATE_ID="$private_id" python3 - <<'PY'
import json
import os

results = json.loads(os.environ["SEARCH_JSON"])
assert all(result["memory_id"] != os.environ["PRIVATE_ID"] for result in results)
PY

cairn project-revoke "$store" "$owner_token" "$project_id" "$promoted_id" member@example.test >/dev/null
after="$(cairn project-shared "$store" "$member_token" "$project_id")"
AFTER_JSON="$after" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["AFTER_JSON"]) == []
PY

echo "Layer 5 CLI smoke check passed"
