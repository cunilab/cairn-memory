#!/bin/sh
set -eu

# A shell that points these at a real server would send this smoke's writes there.
unset CAIRN_URL CAIRN_DEVICE_TOKEN CAIRN_OUTBOX

store="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-store.XXXXXX")"
restored="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-restored.XXXXXX")"
log="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-log.XXXXXX")"
restored_log="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-restored-log.XXXXXX")"
bundle="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-bundle.XXXXXX")"
hook_first="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-hook-first.XXXXXX")"
hook_duplicate="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-hook-duplicate.XXXXXX")"
summary="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-summary.XXXXXX")"
resume="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-resume.XXXXXX")"
unresolved="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-unresolved.XXXXXX")"
search_before="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-search-before.XXXXXX")"
project_memory="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-project-memory.XXXXXX")"
branch_memory="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-branch-memory.XXXXXX")"
preference_memory="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-global-memory.XXXXXX")"
scoped_main="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-scoped-main.XXXXXX")"
scoped_branch="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-scoped-branch.XXXXXX")"
search_after_restart="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-search-after-restart.XXXXXX")"
corrected="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-corrected.XXXXXX")"
search_after_correction="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-search-after-correction.XXXXXX")"
search_after_forget="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-search-after-forget.XXXXXX")"
import_result="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-import-result.XXXXXX")"
search_restored="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-search-restored.XXXXXX")"
pid=""
restored_pid=""
session=""
cli_root=""
port="3115"
restored_port="3116"
base="http://127.0.0.1:${port}"
restored_base="http://127.0.0.1:${restored_port}"

cleanup() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  if [ -n "$restored_pid" ]; then
    kill "$restored_pid" 2>/dev/null || true
    wait "$restored_pid" 2>/dev/null || true
  fi
  rm -f "$store" "$restored" "$log" "$restored_log" "$bundle" \
    "$hook_first" "$hook_duplicate" "$summary" "$resume" "$unresolved" "$search_before" \
    "$project_memory" "$branch_memory" "$preference_memory" "$scoped_main" "$scoped_branch" \
    "$search_after_restart" "$corrected" "$search_after_correction" \
    "$search_after_forget" "$import_result" "$search_restored"
  if [ -n "$cli_root" ]; then
    rm -rf "$cli_root"
  fi
}
trap cleanup EXIT INT TERM
rm -f "$store" "$restored"

wait_for_server() {
  url="$1"
  log_file="$2"
  ready=0
  for _ in $(seq 1 600); do
    if curl -fsS "$url/cairn-memory/health" >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 0.1
  done
  if [ "$ready" -ne 1 ]; then
    printf '%s\n' "server did not become ready: $url" >&2
    cat "$log_file" >&2
    exit 1
  fi
}

start_source_server() {
  cargo run -q -p cairn-server -- --store "$store" --bind "127.0.0.1:${port}" >"$log" 2>&1 &
  pid=$!
  wait_for_server "$base" "$log"
}

# Agents use a device token that the web session creates, so the first account comes first.
json_field() {
  python3 -c 'import json,sys; print(eval("d" + sys.argv[1], {"d": json.load(sys.stdin)}))' "$1"
}

# Prints "<session token> <device token>" for a freshly bootstrapped server.
bootstrap_device() {
  url="$1"
  session="$(curl -fsS -X POST "$url/cairn-memory/auth/bootstrap" \
    -H 'Content-Type: application/json' \
    -d '{"email":"full-flow@example.test","password":"correct horse battery staple"}' | json_field '["token"]')"
  device_token="$(curl -fsS -X POST "$url/cairn-memory/devices/register" \
    -H 'Content-Type: application/json' -H "Authorization: Bearer $session" \
    -d '{"name":"full flow"}' | json_field '["token"]')"
  printf '%s %s\n' "$session" "$device_token"
}

start_source_server

# Fresh file store: the embedded viewer diagnose counts stay at zero before any write.
diagnose="$(curl -fsS "$base/cairn-memory/diagnose")"
DIAGNOSE_JSON="$diagnose" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["DIAGNOSE_JSON"])
assert payload["temporary_observations"] == 0
assert payload["durable_memories"] == 0
PY

tokens="$(bootstrap_device "$base")"
session="$(printf '%s\n' "$tokens" | cut -d' ' -f1)"
device="$(printf '%s\n' "$tokens" | cut -d' ' -f2)"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/session/start" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"full-flow-session","context":{"repository":"cairn-demo","branch":"main"}}' >/dev/null

CAIRN_DEVICE_TOKEN="$device" CAIRN_URL="$base" python3 scripts/cairn_hook.py >"$hook_first" <<'JSON'
{"eventId":"full-flow-event","sessionId":"full-flow-session","content":"The authentication decision passed after the signed cookie test. The next step is to add the missing signature regression test; the issue is not yet verified. api_key=full-flow-secret","role":"tool","source":"PostToolUse","importance":"high","context":{"repository":"cairn-demo","branch":"main"}}
JSON

CAIRN_DEVICE_TOKEN="$device" CAIRN_URL="$base" python3 scripts/cairn_hook.py >"$hook_duplicate" <<'JSON'
{"eventId":"full-flow-event","sessionId":"full-flow-session","content":"The authentication decision passed after the signed cookie test. The next step is to add the missing signature regression test; the issue is not yet verified. api_key=full-flow-secret","role":"tool","source":"PostToolUse","importance":"high","context":{"repository":"cairn-demo","branch":"main"}}
JSON

FIRST_HOOK_JSON="$hook_first" DUPLICATE_HOOK_JSON="$hook_duplicate" python3 - <<'PY'
import json
import os

first = json.load(open(os.environ["FIRST_HOOK_JSON"]))
duplicate = json.load(open(os.environ["DUPLICATE_HOOK_JSON"]))
assert first["accepted"] is True
assert first["duplicate"] is False
assert duplicate["accepted"] is True
assert duplicate["duplicate"] is True
assert "full-flow-secret" not in json.dumps(first)
assert "full-flow-secret" not in json.dumps(duplicate)
PY

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/session/end" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"full-flow-session"}' >"$summary"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/resume" \
  -H 'Content-Type: application/json' \
  -d '{"limit":5,"context":{"repository":"cairn-demo","branch":"main"}}' >"$resume"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/unresolved" \
  -H 'Content-Type: application/json' \
  -d '{"limit":5,"context":{"repository":"cairn-demo","branch":"main"}}' >"$unresolved"

project_id="$(curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/projects/init" \
  -H 'Content-Type: application/json' \
  -d '{"repository":"cairn-demo","name":"Full flow"}' | json_field '["project_id"]')"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"scope":"project","kind":"fact","title":"Project cache convention","summary":"The project cache convention is documented.","reason":"Keep the verified convention available to members.","context":{"project_id":"'"$project_id"'","repository":"cairn-demo","files":["src/cache.rs"],"symbols":["Cache::get"],"commands":["cargo test cache"],"tests":["cache_reads_are_consistent"],"services":["cache"]}}' >"$project_memory"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/sync/push" \
  -H 'Content-Type: application/json' \
  -d '{"eventId":"full-flow-branch-event","sessionId":"full-flow-branch","role":"tool","source":"smoke","content":"The Redis cache experiment is branch-local and not yet verified.","context":{"repository":"cairn-demo","branch":"experiment/redis-cache","worktree":"wt-cache"}}' >/dev/null
curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/session/end" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"full-flow-branch"}' >"$branch_memory"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"scope":"personal","kind":"preference","title":"Evidence-backed cache decisions","summary":"The user prefers evidence-backed cache decisions.","reason":"Use the stated personal preference.","preference":"evidence-backed cache decisions"}' >"$preference_memory"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"cache","limit":10,"context":{"project_id":"'"$project_id"'","repository":"cairn-demo","branch":"main","worktree":"wt-main"}}' >"$scoped_main"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/context" \
  -H 'Content-Type: application/json' \
  -d '{"query":"cache","limit":2,"context":{"project_id":"'"$project_id"'","repository":"cairn-demo","branch":"experiment/redis-cache","worktree":"wt-cache"}}' >"$scoped_branch"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_before"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/export" \
  -H 'Content-Type: application/json' \
  -d '{}' >"$bundle"

SUMMARY_JSON="$summary" RESUME_JSON="$resume" UNRESOLVED_JSON="$unresolved" SEARCH_JSON="$search_before" BUNDLE_JSON="$bundle" PROJECT_JSON="$project_memory" BRANCH_JSON="$branch_memory" PREFERENCE_JSON="$preference_memory" MAIN_JSON="$scoped_main" BRANCH_CONTEXT_JSON="$scoped_branch" python3 - <<'PY'
import json
import os

summary = json.load(open(os.environ["SUMMARY_JSON"]))
resume = json.load(open(os.environ["RESUME_JSON"]))
unresolved = json.load(open(os.environ["UNRESOLVED_JSON"]))
search = json.load(open(os.environ["SEARCH_JSON"]))
bundle = json.load(open(os.environ["BUNDLE_JSON"]))
project = json.load(open(os.environ["PROJECT_JSON"]))
branch = json.load(open(os.environ["BRANCH_JSON"]))
preference_memory = json.load(open(os.environ["PREFERENCE_JSON"]))
main_results = json.load(open(os.environ["MAIN_JSON"]))
branch_pack = json.load(open(os.environ["BRANCH_CONTEXT_JSON"]))
assert summary["kind"] == "session_summary"
assert summary["source_event_ids"] == ["full-flow-event"]
assert "full-flow-secret" not in json.dumps(summary)
assert len(summary["next_steps"]) == 1
assert len(summary["unresolved_work"]) == 1
assert len(resume["results"]) == 1
assert resume["results"][0]["next_steps"] == summary["next_steps"]
assert len(unresolved["results"]) == 1
assert unresolved["results"][0]["unresolved_work"] == summary["unresolved_work"]
assert len(search) == 2
assert {m["memory_id"] for m in search} == {summary["memory_id"], "handoff-" + summary["memory_id"]}
assert all(m["source_event_ids"] == ["full-flow-event"] for m in search)
main_ids = {result["memory_id"] for result in main_results}
assert project["scope"] == "project"
assert branch["scope"] == "personal"
assert "work-note" in branch["tags"]
assert preference_memory["scope"] == "personal"
assert preference_memory["kind"] == "preference"
assert project["context"]["symbols"] == ["Cache::get"]
assert project["context"]["commands"] == ["cargo test cache"]
assert project["context"]["tests"] == ["cache_reads_are_consistent"]
assert project["context"]["services"] == ["cache"]
assert project["memory_id"] in main_ids
assert branch["memory_id"] not in main_ids
assert "handoff-" + branch["memory_id"] not in main_ids
assert main_ids == {project["memory_id"], preference_memory["memory_id"]}
assert len(branch_pack["results"]) == 2
assert branch_pack["omitted_count"] == 2
assert branch_pack["recovery_hint"]
assert {m["memory_id"] for m in branch_pack["results"]} | set(branch_pack["omitted_memory_ids"]) == {
    project["memory_id"], branch["memory_id"], "handoff-" + branch["memory_id"], preference_memory["memory_id"]
}
assert bundle["format"] == "cairn-memory"
assert bundle["version"] == 1
assert "full-flow-secret" not in json.dumps(bundle)
assert len(bundle["observations"]) == 2
assert len(bundle["memories"]) == 5
assert project["memory_id"] not in {m["memory_id"] for m in bundle["memories"]}, "personal export excludes project records"
PY

page="$(curl -fsS "$base/viewer")"
PAGE_HTML="$page" python3 - <<'PY'
import os

page = os.environ["PAGE_HTML"]
for marker in (
    "Cairn / memory field",
    "/cairn-memory/export",
    "/cairn-memory/correct",
    "Forget this",
    "Use the web console for project decisions",
    "Content-Type",
):
    assert marker in page, marker
for removed in ("Share decision", "Revoke share", "Create team", "Delete memory", "/shared"):
    assert removed not in page, removed
PY

kill "$pid"
wait "$pid" 2>/dev/null || true
pid=""
start_source_server

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_after_restart"

MEMORY_ID="$(SUMMARY_JSON="$summary" SEARCH_JSON="$search_after_restart" python3 -c 'import json, os; summary=json.load(open(os.environ["SUMMARY_JSON"])); hits=json.load(open(os.environ["SEARCH_JSON"])); assert any(m["memory_id"]==summary["memory_id"] for m in hits); print(summary["memory_id"])')"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/correct" \
  -H 'Content-Type: application/json' \
  -d "{\"targetId\":\"$MEMORY_ID\",\"content\":\"The authentication decision is verified after the signed cookie test.\",\"lifecycle\":{\"confidence\":95}}" >"$corrected"

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_after_correction"

CORRECTED_JSON="$corrected" SEARCH_JSON="$search_after_correction" python3 - <<'PY'
import json
import os

corrected = json.load(open(os.environ["CORRECTED_JSON"]))
search = json.load(open(os.environ["SEARCH_JSON"]))
assert corrected["kind"] == "correction"
assert corrected["supersedes"]
assert len(search) == 2
assert {m["memory_id"] for m in search} == {corrected["memory_id"], "handoff-" + corrected["supersedes"]}
assert all(m["memory_id"] != corrected["supersedes"] for m in search)
PY

CORRECTED_ID="$(CORRECTED_JSON="$corrected" python3 -c 'import json, os; print(json.load(open(os.environ["CORRECTED_JSON"]))["memory_id"])')"
curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/forget" \
  -H 'Content-Type: application/json' \
  -d "{\"id\":\"$CORRECTED_ID\"}" >/dev/null

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_after_forget"

SEARCH_JSON="$search_after_forget" MEMORY_ID="$MEMORY_ID" CORRECTED_ID="$CORRECTED_ID" python3 - <<'PY'
import json
import os

search = json.load(open(os.environ["SEARCH_JSON"]))
assert len(search) == 2
assert {m["memory_id"] for m in search} == {os.environ["MEMORY_ID"], "handoff-" + os.environ["MEMORY_ID"]}
assert all(m["memory_id"] != os.environ["CORRECTED_ID"] for m in search)
PY

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/export" \
  -H 'Content-Type: application/json' \
  -d '{}' >"$bundle"

cargo run -q -p cairn-server -- --store "$restored" --bind "127.0.0.1:${restored_port}" >"$restored_log" 2>&1 &
restored_pid=$!
wait_for_server "$restored_base" "$restored_log"
restored_device="$(bootstrap_device "$restored_base" | cut -d' ' -f2)"

# An account import binds sessions to the importing account, and a session that names
# another person is rejected, so the restore drops the old person_id.
python3 - "$bundle" "$restored_base" "$import_result" "$restored_device" <<'PY'
import json
import sys
import urllib.request

bundle_path, base, output_path, token = sys.argv[1:]
payload = json.load(open(bundle_path))
for session in payload["sessions"]:
    session.pop("person_id", None)
request = urllib.request.Request(
    base + "/cairn-memory/import",
    data=json.dumps({"bundle": payload}).encode(),
    headers={"Content-Type": "application/json", "Authorization": "Bearer " + token},
    method="POST",
)
with urllib.request.urlopen(request, timeout=10) as response:
    open(output_path, "wb").write(response.read())
PY

curl -fsS -X POST -H "Authorization: Bearer $restored_device" "$restored_base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_restored"

IMPORT_JSON="$import_result" SEARCH_JSON="$search_restored" BUNDLE_JSON="$bundle" MEMORY_ID="$MEMORY_ID" CORRECTED_ID="$CORRECTED_ID" python3 - <<'PY'
import json
import os

result = json.load(open(os.environ["IMPORT_JSON"]))
search = json.load(open(os.environ["SEARCH_JSON"]))
bundle = json.load(open(os.environ["BUNDLE_JSON"]))
assert result["imported_sessions"] == 2
assert result["imported_observations"] == 2
assert result["imported_memories"] == 6
assert os.environ["CORRECTED_ID"] in bundle["forgottenIds"]
assert os.environ["MEMORY_ID"] not in bundle["forgottenIds"]
assert len(search) == 2
assert {m["memory_id"] for m in search} == {os.environ["MEMORY_ID"], "handoff-" + os.environ["MEMORY_ID"]}
PY

# CLI capture, three scopes, and distillation on the same file-store server.
# An unattached capture is personal (ADR 0014 item 0). An initialized checkout
# stores the capture and the session note in the project, visible to members.
cargo build -q -p cairn-client
bin_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug"
cli_root="$(mktemp -d "${TMPDIR:-/tmp}/cairn-full-flow-cli.XXXXXX")"
printf '%s' "$device" >"$cli_root/owner.device"
XDG_CONFIG_HOME="$cli_root/owner" "$bin_dir/cairn" setup "$base" <"$cli_root/owner.device" >/dev/null
owner() { XDG_CONFIG_HOME="$cli_root/owner" "$bin_dir/cairn" "$@"; }
as_member() { XDG_CONFIG_HOME="$cli_root/member" "$bin_dir/cairn" "$@"; }
as_reader() { XDG_CONFIG_HOME="$cli_root/reader" "$bin_dir/cairn" "$@"; }
project_rows() {
  case "$1" in
    owner) owner project-memories "$2" ;;
    member) as_member project-memories "$2" ;;
    reader) as_reader project-memories "$2" ;;
  esac
}

# Recall from outside the product repo. Inside a Git checkout the CLI adds that
# checkout's repository to search, which would hide this unattached capture.
(cd "$cli_root" && owner start cli-session cairn-demo feat/demo wt-demo abc1234 >/dev/null)
(cd "$cli_root" && owner capture cli-session evt-cli "The quartz latch token passed after the isolated signature fix." >/dev/null)
cli_summary="$(cd "$cli_root" && owner end cli-session)"
SUMMARY_JSON="$cli_summary" python3 - <<'PY'
import json
import os

summary = json.loads(os.environ["SUMMARY_JSON"])
assert summary["kind"] == "session_summary", summary
assert summary["scope"] == "personal", summary.get("scope")
assert summary["source_event_ids"] == ["evt-cli"], summary.get("source_event_ids")
assert "quartz" in summary["content"], summary["content"][:800]
PY
before_forget="$(cd "$cli_root" && owner context "quartz latch token" 5)"
case "$before_forget" in
  *evt-cli*) ;;
  *) printf '%s\n' "CLI capture was not recalled" >&2; exit 1 ;;
esac
scoped_context="$(cd "$cli_root" && owner context "quartz latch token" 5 cairn-demo feat/demo wt-demo abc1234)"
SCOPED_CONTEXT_JSON="$scoped_context" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["SCOPED_CONTEXT_JSON"])
assert pack["results"][0]["scope"] == "personal"
PY
resume_cli="$(cd "$cli_root" && owner resume 5 cairn-demo feat/demo wt-demo abc1234)"
RESUME_JSON="$resume_cli" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["RESUME_JSON"])
assert pack["results"][0]["memory_id"].endswith("-cli-session")
PY
why_cli="$(cd "$cli_root" && owner why-changed "quartz latch token" 5 cairn-demo feat/demo wt-demo abc1234)"
WHY_CHANGED_JSON="$why_cli" python3 - <<'PY'
import json
import os

pack = json.loads(os.environ["WHY_CHANGED_JSON"])
assert pack["results"][0]["memory_id"].endswith("-cli-session")
assert pack["results"][0]["source_event_ids"] == ["evt-cli"]
PY
why_again="$(cd "$cli_root" && owner why-changed "quartz latch token" 5 cairn-demo feat/demo wt-demo abc1234)"
WHY_A="$why_cli" WHY_B="$why_again" python3 - <<'PY'
import json
import os

first = [m["memory_id"] for m in json.loads(os.environ["WHY_A"])["results"]]
second = [m["memory_id"] for m in json.loads(os.environ["WHY_B"])["results"]]
assert first == second
assert len(first) == len(set(first))
PY
(cd "$cli_root" && owner forget evt-cli)
after_forget="$(cd "$cli_root" && owner search "quartz latch token" 5)"
case "$after_forget" in
  *evt-cli*) printf '%s\n' "forgotten CLI evidence was recalled" >&2; exit 1 ;;
esac
lesson="$(owner lesson '{"scope":"personal","title":"Keep imports idempotent.","summary":"Keep imports idempotent.","reason":"Preserve tombstones","kind":"preference","preference":"idempotent imports"}')"
LESSON_JSON="$lesson" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["LESSON_JSON"])
assert memory["kind"] == "preference"
assert memory["scope"] == "personal"
PY
lesson_id="$(LESSON_JSON="$lesson" python3 -c 'import json, os; print(json.loads(os.environ["LESSON_JSON"])["memory_id"])')"
corrected_lesson="$(owner correct "$lesson_id" "Keep imports idempotent and preserve tombstones.")"
CORRECTED_JSON="$corrected_lesson" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["CORRECTED_JSON"])
assert memory["kind"] == "correction"
PY

password="correct horse battery staple"
register_agent() {
  name="$1"
  email="$2"
  agent_session="$(curl -fsS -X POST "$base/cairn-memory/auth/register" \
    -H 'Content-Type: application/json' -H "Authorization: Bearer $session" \
    -d "{\"email\":\"$email\",\"password\":\"$password\"}" | json_field '["token"]')"
  agent_device="$(curl -fsS -X POST "$base/cairn-memory/devices/register" \
    -H 'Content-Type: application/json' -H "Authorization: Bearer $agent_session" \
    -d "{\"name\":\"$name\"}" | json_field '["token"]')"
  printf '%s' "$agent_session" >"$cli_root/$name.session"
  printf '%s' "$agent_device" >"$cli_root/$name.device"
  XDG_CONFIG_HOME="$cli_root/$name" "$bin_dir/cairn" setup "$base" <"$cli_root/$name.device" >/dev/null
}
register_agent member member@example.test
register_agent reader reader@example.test

checkout="$cli_root/checkout"
git init -q "$checkout"
git -C "$checkout" remote add origin git@EXAMPLE.TEST:acme/cairn-demo.git
(cd "$checkout" && owner init >/dev/null)
project_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["project_id"])' "$checkout/.cairn/project.json")"
(cd "$checkout" && as_member init) | grep -q 'already initialized, you are a member'
(cd "$checkout" && as_reader capture join-reader reader-event 'reader joined' >/dev/null)
(cd "$checkout" && as_member start source-session example.test/acme/cairn-demo main >/dev/null)
(cd "$checkout" && as_member capture source-session source-event 'shared raw capture because regression passed' >/dev/null)
as_member end source-session >/dev/null
saved="$(cd "$checkout" && as_member save '{"scope":"project","kind":"rule","title":"shared rule","summary":"Bound concurrent writes","reason":"Prevent lost data","rule":"Use transactions","exceptions":"read-only"}')"
rule_id="$(printf '%s' "$saved" | json_field '["memory_id"]')"
decision="$(cd "$checkout" && owner save '{"scope":"project","kind":"decision","title":"Cache decision","summary":"Use bounded writes","reason":"Regression passed","details":"Verified persistence","decision_context":"Concurrent capture","alternatives":["unbounded writes"],"consequences":"Short transactions","evidence":["cargo test"],"context":{"project_id":"'"$project_id"'","repository":"cairn-demo"}}')"
decision_id="$(printf '%s' "$decision" | json_field '["memory_id"]')"
owner project-verify "$project_id" "$decision_id" abc1234 >/dev/null
owner memory "$decision_id" | python3 -c 'import json,sys; m=json.load(sys.stdin); assert m["title"]=="Cache decision" and m["reason"]=="Regression passed" and m["alternatives"]==["unbounded writes"] and m["scope"]=="project"'
corrected_decision="$(owner correct "$decision_id" "The cache decision stays project-scoped after correction.")"
CORRECTED="$corrected_decision" ID="$decision_id" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["CORRECTED"])
assert memory["scope"] == "project" and memory["supersedes"] == os.environ["ID"]
assert memory["title"] == "Cache decision" and memory["alternatives"] == ["unbounded writes"]
PY
owner project-freshness "$project_id" | python3 -c 'import json,sys; assert any(e["action"]=="verification" for e in json.load(sys.stdin))'
for who in owner member reader; do
  rows="$(project_rows "$who" "$project_id")"
  ROWS="$rows" ID="$rule_id" python3 - <<'PY'
import json
import os

rows = json.loads(os.environ["ROWS"])
assert any(m["memory_id"] == os.environ["ID"] and m["rule"] == "Use transactions" for m in rows)
assert any(m["kind"] == "capture" and "shared raw capture" in m["summary"] for m in rows)
assert any(m["kind"] == "session_summary" for m in rows)
PY
done
as_member lesson '{"scope":"personal","title":"concise answers","summary":"Prefer concise answers","reason":"Save attention","kind":"preference","preference":"concise"}' >/dev/null
as_member save '{"scope":"global","title":"lock writes","summary":"Lock shared writes","reason":"Avoid races","kind":"gotcha","cause":"concurrency","fix":"transaction"}' >/dev/null
as_reader memories | python3 -c 'import json,sys; r=json.load(sys.stdin); assert any(m["scope"]=="global" and m["title"]=="lock writes" for m in r); assert not any(m["scope"]=="personal" and "concise answers" in m["title"] for m in r)'
if as_reader correct "$rule_id" bad >/dev/null 2>&1; then
  printf '%s\n' "a reader corrected a project rule" >&2
  exit 1
fi
if as_reader delete-memory "$rule_id" >/dev/null 2>&1; then
  printf '%s\n' "a reader deleted a project rule" >&2
  exit 1
fi
for method in GET POST; do
  status="$(curl -sS -o /dev/null -w '%{http_code}' -X "$method" "$base/cairn-memory/promote" \
    -H 'Content-Type: application/json' -H "Authorization: Bearer $session" -d '{}')"
  [ "$status" = 404 ] || [ "$status" = 405 ]
done
for old in promote promote-preview project-share project-team; do
  if owner "$old" "$project_id" >/dev/null 2>&1; then
    printf '%s\n' "retired command succeeded: $old" >&2
    exit 1
  fi
done
owner delete-memory "$rule_id" >/dev/null
as_reader project-memories "$project_id" | ID="$rule_id" python3 -c 'import json,sys,os; assert not any(m["memory_id"]==os.environ["ID"] for m in json.load(sys.stdin))'

bundle_path="$cli_root/bundle.json"
import_output="$cli_root/import.json"
owner export "$bundle_path" >/dev/null
owner import "$bundle_path" >"$import_output"
OUTPUT_JSON="$(cat "$import_output")" python3 - <<'PY'
import json
import os

result = json.loads(os.environ["OUTPUT_JSON"])
assert result["imported_sessions"] == 0, result
assert result["imported_observations"] == 0, result
assert result["imported_memories"] == 0, result
PY

# Links, a declared contradiction, feedback, and the admin job queue.
cause="$(curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"scope":"personal","kind":"fact","title":"Signed cookies","summary":"Signed cookies stop replay.","reason":"The signature check failed closed.","statement":"Signed cookies stop replay."}')"
effect="$(curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"scope":"personal","kind":"gotcha","title":"Unsigned cookies","summary":"Unsigned cookies were accepted.","reason":"The check was skipped.","symptom":"replayed session","cause":"missing signature","fix":"require the signature"}')"
cause_id="$(printf '%s' "$cause" | json_field '["memory_id"]')"
effect_id="$(printf '%s' "$effect" | json_field '["memory_id"]')"
linked="$(curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/links" \
  -H 'Content-Type: application/json' \
  -d "{\"sourceId\":\"$cause_id\",\"relation\":\"causes\",\"targetId\":\"$effect_id\"}")"
LINKED="$linked" TARGET="$effect_id" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["LINKED"])
assert any(link["relation"] == "causes" and link["target_id"] == os.environ["TARGET"] for link in memory["links"])
PY
graph="$(curl -fsS -H "Authorization: Bearer $device" "$base/cairn-memory/graph?scope=personal")"
GRAPH="$graph" SOURCE="$cause_id" TARGET="$effect_id" python3 - <<'PY'
import json
import os

graph = json.loads(os.environ["GRAPH"])
assert any(
    edge["relation"] == "causes"
    and {edge["source_id"], edge["target_id"]} == {os.environ["SOURCE"], os.environ["TARGET"]}
    for edge in graph["edges"]
)
PY
curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/links/remove" \
  -H 'Content-Type: application/json' \
  -d "{\"sourceId\":\"$cause_id\",\"relation\":\"causes\",\"targetId\":\"$effect_id\"}" \
  | TARGET="$effect_id" python3 -c 'import json,sys,os; assert not any(link["target_id"]==os.environ["TARGET"] for link in json.load(sys.stdin)["links"])'
contradicted="$(curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/contradict" \
  -H 'Content-Type: application/json' \
  -d "{\"contradictingId\":\"$cause_id\",\"contradictedId\":\"$effect_id\"}")"
CONTRADICTED="$contradicted" TARGET="$effect_id" python3 - <<'PY'
import json
import os

memory = json.loads(os.environ["CONTRADICTED"])
assert os.environ["TARGET"] in memory["contradicts"]
PY
voted="$(curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/feedback" \
  -H 'Content-Type: application/json' \
  -d "{\"memoryId\":\"$cause_id\",\"vote\":\"helpful\"}")"
VOTED="$voted" python3 -c 'import json,os,sys; assert json.loads(os.environ["VOTED"])["helpful_count"] == 1'

curl -fsS -X POST -H "Authorization: Bearer $device" "$base/cairn-memory/session/start" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"empty-job"}' >/dev/null
empty_status="$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $device" \
  "$base/cairn-memory/session/end" -H 'Content-Type: application/json' \
  -d '{"sessionId":"empty-job"}')"
[ "$empty_status" = 400 ]
jobs="$(curl -fsS -H "Authorization: Bearer $session" "$base/cairn-memory/admin/jobs")"
JOBS="$jobs" python3 - <<'PY'
import json
import os

queue = json.loads(os.environ["JOBS"])
assert queue["quarantined"] >= 1
assert queue["quarantine"]
PY
member_jobs="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(cat "$cli_root/member.session")" \
  "$base/cairn-memory/admin/jobs")"
[ "$member_jobs" = 403 ]

echo "Full flow smoke check passed"
