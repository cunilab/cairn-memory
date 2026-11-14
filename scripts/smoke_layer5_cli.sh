#!/bin/sh
set -eu

. "$(dirname "$0")/smoke_agent_lib.sh"

as_member() { XDG_CONFIG_HOME="$smoke_dir/member" "$@"; }

# The memory's repository comes from start, capture, and end. capture copies
# the session context onto the observation, and end merges that observation
# context. end_session does not copy the session repository by itself.

start_server 3133
create_agent owner owner@example.test bootstrap
create_agent member member@example.test register
use_agent owner

project="$(cairn project-register cairn-demo "Cairn Demo" main)"
project_id="$(PROJECT_JSON="$project" python3 -c 'import json, os; print(json.loads(os.environ["PROJECT_JSON"])["project_id"])')"

cairn start private-session >/dev/null
cairn capture private-session evt-private "private observation stays on the owner account" >/dev/null
private_ended="$(cairn end private-session)"
private_id="$(PRIVATE_JSON="$private_ended" python3 -c 'import json, os; print(json.loads(os.environ["PRIVATE_JSON"])["memory_id"])')"

cairn start layer5-session cairn-demo "" "" abc1234 >/dev/null
cairn capture layer5-session evt-layer5 "repository cache decision is ready for the project" >/dev/null
ended="$(cairn end layer5-session)"
memory_id="$(ENDED_JSON="$ended" python3 -c 'import json, os; print(json.loads(os.environ["ENDED_JSON"])["memory_id"])')"

promoted="$(cairn promote "$project_id" "$memory_id" "Share the promoted repository decision")"
promoted_id="$(PROMOTED_JSON="$promoted" python3 -c 'import json, os; print(json.loads(os.environ["PROMOTED_JSON"])["memory_id"])')"

cairn project-invite "$project_id" member@example.test >/dev/null
cairn project-share "$project_id" "$promoted_id" member@example.test >/dev/null

shared="$(as_member cairn project-shared "$project_id")"
SHARED_JSON="$shared" PROMOTED_ID="$promoted_id" PRIVATE_ID="$private_id" python3 - <<'PY'
import json
import os

memories = json.loads(os.environ["SHARED_JSON"])
assert [memory["memory_id"] for memory in memories] == [os.environ["PROMOTED_ID"]]
assert all(memory["memory_id"] != os.environ["PRIVATE_ID"] for memory in memories)
PY

search="$(cairn search "private observation" 5 cairn-demo)"
SEARCH_JSON="$search" PRIVATE_ID="$private_id" python3 - <<'PY'
import json
import os

results = json.loads(os.environ["SEARCH_JSON"])
assert all(result["memory_id"] != os.environ["PRIVATE_ID"] for result in results)
PY

cairn project-revoke "$project_id" "$promoted_id" member@example.test >/dev/null
after="$(as_member cairn project-shared "$project_id")"
AFTER_JSON="$after" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["AFTER_JSON"]) == []
PY

cairn project-share "$project_id" "$promoted_id" member@example.test >/dev/null
team="$(cairn project-team-create "$project_id" Platform)"
TEAM_JSON="$team" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["TEAM_JSON"])["name"] == "Platform"
PY
cairn project-team-add "$project_id" member@example.test >/dev/null
listed="$(as_member cairn project-team "$project_id")"
LISTED_JSON="$listed" python3 - <<'PY'
import json
import os

team = json.loads(os.environ["LISTED_JSON"])
assert team["name"] == "Platform"
assert [member["email"] for member in team["members"]] == ["member@example.test"]
PY
if as_member cairn project-team-create "$project_id" Other >/dev/null 2>&1; then
  printf '%s\n' "member team create should fail" >&2
  exit 1
fi
# Removing a member is web-only: the owner's device token is refused, the web session works.
remove_url="$base/cairn-memory/projects/$project_id/team/members/remove"
remove_body='{"email":"member@example.test"}'
status="$(curl -sS -o "$smoke_dir/remove.out" -w '%{http_code}' -X POST "$remove_url" \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $(device_of owner)" -d "$remove_body")"
if [ "$status" != "403" ] || ! grep -q "device tokens cannot do this" "$smoke_dir/remove.out"; then
  printf '%s\n' "device token should be refused with 403 on team member removal" >&2
  exit 1
fi
curl -fsS -X POST "$remove_url" -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $(session_of owner)" -d "$remove_body" >/dev/null
former="$(as_member cairn project-shared "$project_id")"
FORMER_JSON="$former" python3 - <<'PY'
import json
import os

assert json.loads(os.environ["FORMER_JSON"]) == []
PY
cairn delete-memory "$promoted_id" >/dev/null
export_path="$smoke_dir/export.json"
cairn export "$export_path"
EXPORT_PATH="$export_path" PROMOTED_ID="$promoted_id" python3 - <<'PY'
import json
import os

bundle = json.load(open(os.environ["EXPORT_PATH"]))
promoted_id = os.environ["PROMOTED_ID"]
assert any(memory["memory_id"] == promoted_id for memory in bundle["memories"])
assert promoted_id in bundle["forgottenIds"]
PY

echo "Layer 5 CLI smoke check passed"
