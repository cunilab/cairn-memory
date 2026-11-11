#!/bin/sh
set -eu

cd "$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"

workdir="$(mktemp -d "${TMPDIR:-/tmp}/cairn-web.XXXXXX")"
store="$workdir/store.json"
api_log="$workdir/api.log"
web_log="$workdir/web.log"
api_pid=""
web_pid=""

cleanup() {
  if [ -n "$web_pid" ]; then
    kill "$web_pid" 2>/dev/null || true
    pkill -P "$web_pid" 2>/dev/null || true
  fi
  if [ -n "$api_pid" ]; then
    kill "$api_pid" 2>/dev/null || true
    pkill -P "$api_pid" 2>/dev/null || true
  fi
  if [ -n "${web_port:-}" ]; then
    web_listen="$(lsof -nP -tiTCP:"$web_port" -sTCP:LISTEN 2>/dev/null || true)"
    if [ -n "$web_listen" ]; then
      # shellcheck disable=SC2086
      kill $web_listen 2>/dev/null || true
    fi
  fi
  if [ -n "${api_port:-}" ]; then
    api_listen="$(lsof -nP -tiTCP:"$api_port" -sTCP:LISTEN 2>/dev/null || true)"
    if [ -n "$api_listen" ]; then
      # shellcheck disable=SC2086
      kill $api_listen 2>/dev/null || true
    fi
  fi
  if [ -n "$web_pid" ]; then
    wait "$web_pid" 2>/dev/null || true
  fi
  if [ -n "$api_pid" ]; then
    wait "$api_pid" 2>/dev/null || true
  fi
  rm -rf "$workdir"
}
trap cleanup EXIT INT TERM

ports="$(python3 - <<'PY'
import socket
def free():
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port
print(free())
print(free())
PY
)"
api_port="$(printf '%s\n' "$ports" | sed -n '1p')"
web_port="$(printf '%s\n' "$ports" | sed -n '2p')"

node --experimental-strip-types --disable-warning=MODULE_TYPELESS_PACKAGE_JSON --input-type=module <<'JS'
import { memoryReadPath } from "./apps/web/lib/memory-path.ts";

const member = memoryReadPath("proj", "member");
const owner = memoryReadPath("proj", "owner");
if (member !== "projects/proj/shared") {
  throw new Error(`member path ${member}`);
}
if (owner !== "projects/proj/memories") {
  throw new Error(`owner path ${owner}`);
}
if (member.includes("memories")) {
  throw new Error("member path requests owner memories");
}
JS

if [ ! -d apps/web/node_modules/next ]; then
  npm install --prefix apps/web
fi

cargo run -q -p cairn-server -- --store "$store" --bind "127.0.0.1:${api_port}" >"$api_log" 2>&1 &
api_pid=$!

ready=0
for _ in $(seq 1 1800); do
  if curl -fsS "http://127.0.0.1:${api_port}/cairn-memory/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if [ "$ready" -ne 1 ]; then
  printf '%s\n' "cairn-server did not become ready" >&2
  cat "$api_log" >&2
  exit 1
fi

api="http://127.0.0.1:${api_port}"
password="correct horse battery staple"

owner_json="$(curl -fsS -H 'content-type: application/json' \
  -d "{\"email\":\"owner@example.test\",\"password\":\"${password}\"}" \
  "${api}/cairn-memory/auth/bootstrap")"
owner_token="$(printf '%s' "$owner_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"

curl -fsS -H 'content-type: application/json' \
  -d "{\"email\":\"member@example.test\",\"password\":\"${password}\"}" \
  "${api}/cairn-memory/auth/register" >/dev/null

project_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${owner_token}" \
  -d '{"repository":"cairn-demo","name":"Cairn Demo","default_branch":"main"}' \
  "${api}/cairn-memory/projects")"
project_id="$(printf '%s' "$project_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["project_id"])')"

curl -fsS -H 'content-type: application/json' \
  -d '{"session_id":"web-private","context":{}}' \
  "${api}/cairn-memory/session/start" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -d '{"event_id":"evt-private","session_id":"web-private","content":"private observation stays on the owner account","role":"agent","source":"hook"}' \
  "${api}/cairn-memory/observe" >/dev/null
private_json="$(curl -fsS -H 'content-type: application/json' \
  -d '{"session_id":"web-private"}' \
  "${api}/cairn-memory/session/end")"
private_id="$(printf '%s' "$private_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_id"])')"

curl -fsS -H 'content-type: application/json' \
  -d '{"session_id":"web-project","context":{"repository":"cairn-demo","commit":"abc1234"}}' \
  "${api}/cairn-memory/session/start" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -d '{"event_id":"evt-project","session_id":"web-project","content":"repository cache decision is ready for the project","role":"agent","source":"hook"}' \
  "${api}/cairn-memory/observe" >/dev/null
ended_json="$(curl -fsS -H 'content-type: application/json' \
  -d '{"session_id":"web-project"}' \
  "${api}/cairn-memory/session/end")"
source_id="$(printf '%s' "$ended_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_id"])')"

promoted_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${owner_token}" \
  -d "{\"project_id\":\"${project_id}\",\"memory_id\":\"${source_id}\",\"reason\":\"Share the promoted repository decision\"}" \
  "${api}/cairn-memory/promote")"
promoted_id="$(printf '%s' "$promoted_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_id"])')"

curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${owner_token}" \
  -d '{"email":"member@example.test"}' \
  "${api}/cairn-memory/projects/${project_id}/members" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${owner_token}" \
  -d "{\"memory_id\":\"${promoted_id}\",\"email\":\"member@example.test\"}" \
  "${api}/cairn-memory/projects/${project_id}/shares" >/dev/null

(
  cd apps/web
  export CAIRN_API_URL="$api"
  export NEXT_TELEMETRY_DISABLED=1
  exec ./node_modules/.bin/next dev --hostname 127.0.0.1 --port "$web_port"
) >"$web_log" 2>&1 &
web_pid=$!

web_ready=0
for _ in $(seq 1 1800); do
  if curl -fsS "http://127.0.0.1:${web_port}/" >/dev/null 2>&1; then
    web_ready=1
    break
  fi
  sleep 0.1
done
if [ "$web_ready" -ne 1 ]; then
  printf '%s\n' "Next.js console did not become ready" >&2
  cat "$web_log" >&2
  exit 1
fi

web="http://127.0.0.1:${web_port}"
owner_headers="$workdir/owner.headers"
owner_body="$workdir/owner.body"
owner_jar="$workdir/owner.cookies"
member_headers="$workdir/member.headers"
member_body="$workdir/member.body"
member_jar="$workdir/member.cookies"

owner_code="$(curl -sS -o "$owner_body" -D "$owner_headers" -c "$owner_jar" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d "{\"email\":\"owner@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/login")"
if [ "$owner_code" != "200" ]; then
  printf '%s\n' "owner login failed: ${owner_code}" >&2
  cat "$owner_body" >&2
  exit 1
fi

OWNER_HEADERS="$owner_headers" OWNER_BODY="$owner_body" python3 - <<'PY'
import os
headers = open(os.environ["OWNER_HEADERS"], encoding="utf-8").read().lower()
body = open(os.environ["OWNER_BODY"], encoding="utf-8").read()
if "set-cookie:" not in headers or "cairn_token=" not in headers or "httponly" not in headers:
    raise SystemExit("login did not set an httpOnly cairn_token cookie")
if '"token"' in body:
    raise SystemExit("login response still contains a token field")
PY

projects_body="$workdir/projects.body"
projects_code="$(curl -sS -o "$projects_body" -b "$owner_jar" -w '%{http_code}' \
  "${web}/api/cairn/projects")"
if [ "$projects_code" != "200" ]; then
  printf '%s\n' "project list failed: ${projects_code}" >&2
  cat "$projects_body" >&2
  exit 1
fi
PROJECTS_BODY="$projects_body" PROJECT_ID="$project_id" python3 - <<'PY'
import json
import os
projects = json.load(open(os.environ["PROJECTS_BODY"], encoding="utf-8"))
match = [item for item in projects if item["project_id"] == os.environ["PROJECT_ID"]]
assert len(match) == 1
assert match[0]["role"] == "owner"
assert match[0]["name"] == "Cairn Demo"
PY

member_code="$(curl -sS -o "$member_body" -D "$member_headers" -c "$member_jar" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d "{\"email\":\"member@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/login")"
if [ "$member_code" != "200" ]; then
  printf '%s\n' "member login failed: ${member_code}" >&2
  cat "$member_body" >&2
  exit 1
fi

member_projects="$workdir/member-projects.body"
member_projects_code="$(curl -sS -o "$member_projects" -b "$member_jar" -w '%{http_code}' \
  "${web}/api/cairn/projects")"
if [ "$member_projects_code" != "200" ]; then
  printf '%s\n' "member project list failed: ${member_projects_code}" >&2
  cat "$member_projects" >&2
  exit 1
fi
MEMBER_PROJECTS="$member_projects" PROJECT_ID="$project_id" python3 - <<'PY'
import json
import os
projects = json.load(open(os.environ["MEMBER_PROJECTS"], encoding="utf-8"))
match = [item for item in projects if item["project_id"] == os.environ["PROJECT_ID"]]
assert len(match) == 1
assert match[0]["role"] == "member"
PY

shared_body="$workdir/shared.body"
shared_code="$(curl -sS -o "$shared_body" -b "$member_jar" -w '%{http_code}' \
  "${web}/api/cairn/projects/${project_id}/shared")"
if [ "$shared_code" != "200" ]; then
  printf '%s\n' "member shared read failed: ${shared_code}" >&2
  cat "$shared_body" >&2
  exit 1
fi
SHARED_BODY="$shared_body" PROMOTED_ID="$promoted_id" PRIVATE_ID="$private_id" python3 - <<'PY'
import json
import os
memories = json.load(open(os.environ["SHARED_BODY"], encoding="utf-8"))
ids = [memory["memory_id"] for memory in memories]
assert ids == [os.environ["PROMOTED_ID"]]
assert os.environ["PRIVATE_ID"] not in ids
blob = json.dumps(memories)
assert "private observation stays on the owner account" not in blob
PY

echo "Web console smoke check passed"
