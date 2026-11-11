#!/bin/sh
set -eu

cd "$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"

store="$(mktemp "${TMPDIR:-/tmp}/cairn-layer5.XXXXXX")"
export_path=""
trap 'rm -f "$store" "$export_path"' EXIT
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

cairn project-share "$store" "$owner_token" "$project_id" "$promoted_id" member@example.test >/dev/null
team="$(cairn project-team-create "$store" "$owner_token" "$project_id" Platform)"
TEAM_JSON="$team" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["TEAM_JSON"])["name"] == "Platform"
PY
cairn project-team-add "$store" "$owner_token" "$project_id" member@example.test >/dev/null
listed="$(cairn project-team "$store" "$member_token" "$project_id")"
LISTED_JSON="$listed" python3 - <<'PY'
import json
import os

team = json.loads(os.environ["LISTED_JSON"])
assert team["name"] == "Platform"
assert [member["email"] for member in team["members"]] == ["member@example.test"]
PY
if cairn project-team-create "$store" "$member_token" "$project_id" Other >/dev/null 2>&1; then
  printf '%s\n' "member team create should fail" >&2
  exit 1
fi
cairn project-team-remove "$store" "$owner_token" "$project_id" member@example.test >/dev/null
former="$(cairn project-shared "$store" "$member_token" "$project_id")"
FORMER_JSON="$former" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["FORMER_JSON"]) == []
PY
cairn delete-memory "$store" "$owner_token" "$promoted_id" >/dev/null
export_path="$(mktemp "${TMPDIR:-/tmp}/cairn-layer5-export.XXXXXX")"
cairn export "$store" "$export_path"
EXPORT_PATH="$export_path" PROMOTED_ID="$promoted_id" python3 - <<'PY'
import json
import os

bundle = json.load(open(os.environ["EXPORT_PATH"]))
promoted_id = os.environ["PROMOTED_ID"]
assert any(memory["memory_id"] == promoted_id for memory in bundle["memories"])
assert promoted_id in bundle["forgottenIds"]
PY

echo "Layer 5 CLI smoke check passed"
