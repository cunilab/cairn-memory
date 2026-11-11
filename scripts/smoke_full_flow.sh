#!/bin/sh
set -eu

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
global_memory="$(mktemp "${TMPDIR:-/tmp}/cairn-full-flow-global-memory.XXXXXX")"
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
    "$project_memory" "$branch_memory" "$global_memory" "$scoped_main" "$scoped_branch" \
    "$search_after_restart" "$corrected" "$search_after_correction" \
    "$search_after_forget" "$import_result" "$search_restored"
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

start_source_server

curl -fsS -X POST "$base/cairn-memory/session/start" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"full-flow-session","context":{"repository":"cairn-demo","branch":"main"}}' >/dev/null

CAIRN_URL="$base" python3 scripts/cairn_hook.py >"$hook_first" <<'JSON'
{"eventId":"full-flow-event","sessionId":"full-flow-session","content":"The authentication decision passed after the signed cookie test. The next step is to add the missing signature regression test; the issue is not yet verified. api_key=full-flow-secret","role":"tool","source":"PostToolUse","importance":"high","context":{"repository":"cairn-demo","branch":"main"}}
JSON

CAIRN_URL="$base" python3 scripts/cairn_hook.py >"$hook_duplicate" <<'JSON'
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

curl -fsS -X POST "$base/cairn-memory/session/end" \
  -H 'Content-Type: application/json' \
  -d '{"sessionId":"full-flow-session"}' >"$summary"

curl -fsS -X POST "$base/cairn-memory/resume" \
  -H 'Content-Type: application/json' \
  -d '{"limit":5,"context":{"repository":"cairn-demo","branch":"main"}}' >"$resume"

curl -fsS -X POST "$base/cairn-memory/unresolved" \
  -H 'Content-Type: application/json' \
  -d '{"limit":5,"context":{"repository":"cairn-demo","branch":"main"}}' >"$unresolved"

curl -fsS -X POST "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"content":"The project cache convention is documented.","context":{"repository":"cairn-demo","files":["src/cache.rs"],"symbols":["Cache::get"],"commands":["cargo test cache"],"tests":["cache_reads_are_consistent"],"services":["cache"]}}' >"$project_memory"

curl -fsS -X POST "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"content":"The Redis cache experiment is branch-local and not yet verified.","context":{"repository":"cairn-demo","branch":"experiment/redis-cache","worktree":"wt-cache"}}' >"$branch_memory"

curl -fsS -X POST "$base/cairn-memory/remember" \
  -H 'Content-Type: application/json' \
  -d '{"content":"The user prefers evidence-backed cache decisions."}' >"$global_memory"

curl -fsS -X POST "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"cache","limit":10,"context":{"repository":"cairn-demo","branch":"main","worktree":"wt-main"}}' >"$scoped_main"

curl -fsS -X POST "$base/cairn-memory/context" \
  -H 'Content-Type: application/json' \
  -d '{"query":"cache","limit":2,"context":{"repository":"cairn-demo","branch":"experiment/redis-cache","worktree":"wt-cache"}}' >"$scoped_branch"

curl -fsS -X POST "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_before"

curl -fsS -X POST "$base/cairn-memory/export" \
  -H 'Content-Type: application/json' \
  -d '{}' >"$bundle"

SUMMARY_JSON="$summary" RESUME_JSON="$resume" UNRESOLVED_JSON="$unresolved" SEARCH_JSON="$search_before" BUNDLE_JSON="$bundle" PROJECT_JSON="$project_memory" BRANCH_JSON="$branch_memory" GLOBAL_JSON="$global_memory" MAIN_JSON="$scoped_main" BRANCH_CONTEXT_JSON="$scoped_branch" python3 - <<'PY'
import json
import os

summary = json.load(open(os.environ["SUMMARY_JSON"]))
resume = json.load(open(os.environ["RESUME_JSON"]))
unresolved = json.load(open(os.environ["UNRESOLVED_JSON"]))
search = json.load(open(os.environ["SEARCH_JSON"]))
bundle = json.load(open(os.environ["BUNDLE_JSON"]))
project = json.load(open(os.environ["PROJECT_JSON"]))
branch = json.load(open(os.environ["BRANCH_JSON"]))
global_memory = json.load(open(os.environ["GLOBAL_JSON"]))
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
assert len(search) == 1
assert search[0]["source_event_ids"] == ["full-flow-event"]
main_ids = {result["memory_id"] for result in main_results}
assert project["scope"] == "project"
assert branch["scope"] == "worktree"
assert project["context"]["symbols"] == ["Cache::get"]
assert project["context"]["commands"] == ["cargo test cache"]
assert project["context"]["tests"] == ["cache_reads_are_consistent"]
assert project["context"]["services"] == ["cache"]
assert project["memory_id"] in main_ids
assert branch["memory_id"] not in main_ids
assert len(branch_pack["results"]) == 2
assert branch_pack["omitted_count"] == 1
assert branch_pack["recovery_hint"]
assert global_memory["memory_id"] in branch_pack["omitted_memory_ids"]
assert bundle["format"] == "cairn-memory"
assert bundle["version"] == 1
assert "full-flow-secret" not in json.dumps(bundle)
assert len(bundle["observations"]) == 1
assert len(bundle["memories"]) == 4
PY

page="$(curl -fsS "$base/viewer")"
PAGE_HTML="$page" python3 - <<'PY'
import os

page = os.environ["PAGE_HTML"]
for marker in ("Cairn / memory field", "/cairn-memory/correct", "Forget this"):
    assert marker in page, marker
PY

kill "$pid"
wait "$pid" 2>/dev/null || true
pid=""
start_source_server

curl -fsS -X POST "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_after_restart"

MEMORY_ID="$(SEARCH_JSON="$search_after_restart" python3 -c 'import json, os; print(json.load(open(os.environ["SEARCH_JSON"]))[0]["memory_id"])')"

curl -fsS -X POST "$base/cairn-memory/correct" \
  -H 'Content-Type: application/json' \
  -d "{\"targetId\":\"$MEMORY_ID\",\"content\":\"The authentication decision is verified after the signed cookie test.\",\"lifecycle\":{\"confidence\":95}}" >"$corrected"

curl -fsS -X POST "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_after_correction"

CORRECTED_JSON="$corrected" SEARCH_JSON="$search_after_correction" python3 - <<'PY'
import json
import os

corrected = json.load(open(os.environ["CORRECTED_JSON"]))
search = json.load(open(os.environ["SEARCH_JSON"]))
assert corrected["kind"] == "correction"
assert corrected["supersedes"]
assert len(search) == 1
assert search[0]["memory_id"] == corrected["memory_id"]
assert search[0]["memory_id"] != corrected["supersedes"]
PY

CORRECTED_ID="$(CORRECTED_JSON="$corrected" python3 -c 'import json, os; print(json.load(open(os.environ["CORRECTED_JSON"]))["memory_id"])')"
curl -fsS -X POST "$base/cairn-memory/forget" \
  -H 'Content-Type: application/json' \
  -d "{\"id\":\"$CORRECTED_ID\"}" >/dev/null

curl -fsS -X POST "$base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_after_forget"

SEARCH_JSON="$search_after_forget" MEMORY_ID="$MEMORY_ID" CORRECTED_ID="$CORRECTED_ID" python3 - <<'PY'
import json
import os

search = json.load(open(os.environ["SEARCH_JSON"]))
assert len(search) == 1
assert search[0]["memory_id"] == os.environ["MEMORY_ID"]
assert search[0]["memory_id"] != os.environ["CORRECTED_ID"]
PY

curl -fsS -X POST "$base/cairn-memory/export" \
  -H 'Content-Type: application/json' \
  -d '{}' >"$bundle"

cargo run -q -p cairn-server -- --store "$restored" --bind "127.0.0.1:${restored_port}" >"$restored_log" 2>&1 &
restored_pid=$!
wait_for_server "$restored_base" "$restored_log"

python3 - "$bundle" "$restored_base" "$import_result" <<'PY'
import json
import sys
import urllib.request

bundle_path, base, output_path = sys.argv[1:]
payload = json.load(open(bundle_path))
request = urllib.request.Request(
    base + "/cairn-memory/import",
    data=json.dumps({"bundle": payload}).encode(),
    headers={"Content-Type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(request, timeout=10) as response:
    open(output_path, "wb").write(response.read())
PY

curl -fsS -X POST "$restored_base/cairn-memory/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"authentication signed cookie","limit":5}' >"$search_restored"

IMPORT_JSON="$import_result" SEARCH_JSON="$search_restored" BUNDLE_JSON="$bundle" MEMORY_ID="$MEMORY_ID" CORRECTED_ID="$CORRECTED_ID" python3 - <<'PY'
import json
import os

result = json.load(open(os.environ["IMPORT_JSON"]))
search = json.load(open(os.environ["SEARCH_JSON"]))
bundle = json.load(open(os.environ["BUNDLE_JSON"]))
assert result["imported_sessions"] == 1
assert result["imported_observations"] == 1
assert result["imported_memories"] == 5
assert os.environ["CORRECTED_ID"] in bundle["forgottenIds"]
assert os.environ["MEMORY_ID"] not in bundle["forgottenIds"]
assert len(search) == 1
assert search[0]["memory_id"] == os.environ["MEMORY_ID"]
PY

echo "Full Layer 1/2 acceptance flow passed"
