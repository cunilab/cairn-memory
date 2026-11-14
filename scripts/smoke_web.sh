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

node --experimental-strip-types --disable-warning=MODULE_TYPELESS_PACKAGE_JSON --input-type=module <<'JS'
import {
  isNoBodyStatus,
  isSessionRequired,
  signInFailureMessage,
} from "./apps/web/lib/session.ts";

for (const status of [101, 204, 205, 304]) {
  if (!isNoBodyStatus(status)) {
    throw new Error(`expected no body for ${status}`);
  }
}
for (const status of [200, 400, 401, 500]) {
  if (isNoBodyStatus(status)) {
    throw new Error(`status ${status} can carry a body`);
  }
}
for (const message of [
  "Bearer token is required",
  "authentication failed",
  "account authentication failed",
  "account session is expired or revoked",
]) {
  if (!isSessionRequired(401, message)) {
    throw new Error(`expected sign-in redirect for ${message}`);
  }
}
if (isSessionRequired(401, "invalid account credentials")) {
  throw new Error("a failed sign-in must stay on the form");
}
if (isSessionRequired(401, "does not own this project")) {
  throw new Error("an authorization failure must not redirect to sign-in");
}
if (isSessionRequired(500, "Bearer token is required")) {
  throw new Error("a server failure must not redirect to sign-in");
}
if (signInFailureMessage("invalid account credentials") !== "Email or password is incorrect.") {
  throw new Error("credential failure copy");
}
if (signInFailureMessage("authentication rate limit exceeded") !== "authentication rate limit exceeded") {
  throw new Error("unrelated auth errors must pass through");
}
JS

node --experimental-strip-types --disable-warning=MODULE_TYPELESS_PACKAGE_JSON --input-type=module <<'JS'
import { isAbsentTeam } from "./apps/web/lib/team.ts";

if (!isAbsentTeam("validation error: project has no team space")) {
  throw new Error("a project with no team must be the empty state");
}
for (const message of [
  "validation error: project already has a team space",
  "validation error: account is not a project member",
  "validation error: unknown project: project-1",
  "project has no team space",
  "database unavailable",
  "",
]) {
  if (isAbsentTeam(message)) {
    throw new Error(`team failure must stay visible: ${message}`);
  }
}
JS

node --experimental-strip-types --disable-warning=MODULE_TYPELESS_PACKAGE_JSON --input-type=module <<'JS'
import { deleteDecisionImpact, revokeShareImpact } from "./apps/web/lib/action-copy.ts";

const deleted = deleteDecisionImpact("memory-1");
if (!deleted.includes("memory-1") || !deleted.includes("retained delete") || !deleted.includes("keeps the stored record")) {
  throw new Error("delete copy must describe a retained hide");
}
if (/permanently|erase|purged|undo button/i.test(deleted)) {
  throw new Error("delete copy must not promise erasure or an undo control");
}
const revoked = revokeShareImpact("memory-1", "member@example.test");
if (!revoked.includes("member@example.test") || !revoked.includes("stays on your account")) {
  throw new Error("revoke copy must name the member and keep the decision");
}
if (!revoked.includes("Share this decision again to show it to this member.")) {
  throw new Error("revoke copy must say sharing again restores access");
}
if (/cannot undo the revoke/i.test(revoked)) {
  throw new Error("revoke copy must not say the console cannot undo a revoke");
}
const missingEmail = revokeShareImpact("memory-1", "  ");
if (!missingEmail.includes("member email") || !missingEmail.includes("No request is sent")) {
  throw new Error("revoke without an email must not describe a completed change");
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

health_json="$(curl -fsS "${api}/cairn-memory/health")"
printf '%s' "$health_json" | python3 -c 'import json,sys; assert json.load(sys.stdin)["bootstrap_required"] is True'

owner_json="$(curl -fsS -H 'content-type: application/json' \
  -d "{\"email\":\"owner@example.test\",\"password\":\"${password}\"}" \
  "${api}/cairn-memory/auth/bootstrap")"
owner_token="$(printf '%s' "$owner_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
curl -fsS "${api}/cairn-memory/health" \
  | python3 -c 'import json,sys; assert json.load(sys.stdin)["bootstrap_required"] is False'

# Agent work below uses a device token that the owner session creates.
device_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${owner_token}" \
  -d '{"name":"smoke agent"}' \
  "${api}/cairn-memory/devices/register")"
device_token="$(printf '%s' "$device_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
for web_only in "GET devices" "POST devices/revoke" "GET auth/audit"; do
  device_denied="$(curl -sS -o /dev/null -w '%{http_code}' -X "${web_only%% *}" \
    -H 'content-type: application/json' -H "authorization: Bearer ${device_token}" \
    -d '{"deviceId":"x"}' "${api}/cairn-memory/${web_only#* }")"
  if [ "$device_denied" != "403" ]; then
    printf '%s\n' "device token on ${web_only}: ${device_denied}" >&2
    exit 1
  fi
done

curl -fsS -H 'content-type: application/json' \
  -d "{\"email\":\"member@example.test\",\"password\":\"${password}\"}" \
  "${api}/cairn-memory/auth/register" >/dev/null

project_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"repository":"cairn-demo","name":"Cairn Demo","default_branch":"main"}' \
  "${api}/cairn-memory/projects")"
project_id="$(printf '%s' "$project_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["project_id"])')"

curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"session_id":"web-private","context":{}}' \
  "${api}/cairn-memory/session/start" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"event_id":"evt-private","session_id":"web-private","content":"private observation stays on the owner account","role":"agent","source":"hook"}' \
  "${api}/cairn-memory/observe" >/dev/null
private_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"session_id":"web-private"}' \
  "${api}/cairn-memory/session/end")"
private_id="$(printf '%s' "$private_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_id"])')"

curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"session_id":"web-project","context":{"repository":"cairn-demo","commit":"abc1234"}}' \
  "${api}/cairn-memory/session/start" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"event_id":"evt-project","session_id":"web-project","content":"repository cache decision is ready for the project","role":"agent","source":"hook"}' \
  "${api}/cairn-memory/observe" >/dev/null
ended_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"session_id":"web-project"}' \
  "${api}/cairn-memory/session/end")"
source_id="$(printf '%s' "$ended_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_id"])')"

promoted_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d "{\"project_id\":\"${project_id}\",\"memory_id\":\"${source_id}\",\"reason\":\"Share the promoted repository decision\"}" \
  "${api}/cairn-memory/promote")"
promoted_id="$(printf '%s' "$promoted_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_id"])')"

curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"email":"member@example.test"}' \
  "${api}/cairn-memory/projects/${project_id}/members" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
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

register_headers="$workdir/register.headers"
register_body="$workdir/register.body"
register_code="$(curl -sS -o "$register_body" -D "$register_headers" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d "{\"email\":\"proxy-register@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/register")"
if [ "$register_code" != "200" ]; then
  printf '%s\n' "register through the proxy failed: ${register_code}" >&2
  cat "$register_body" >&2
  exit 1
fi
REGISTER_HEADERS="$register_headers" REGISTER_BODY="$register_body" python3 - <<'PY'
import os
headers = open(os.environ["REGISTER_HEADERS"], encoding="utf-8").read().lower()
body = open(os.environ["REGISTER_BODY"], encoding="utf-8").read()
if "cairn_token=" not in headers or "httponly" not in headers:
    raise SystemExit("register did not set an httpOnly cairn_token cookie")
if '"token"' in body:
    raise SystemExit("register response still contains a token field")
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

devices_page="$(curl -sS -o /dev/null -w '%{http_code}' -b "$owner_jar" "${web}/devices")"
if [ "$devices_page" != "200" ]; then
  printf '%s\n' "devices page status: ${devices_page}" >&2
  exit 1
fi
created_body="$workdir/device-created.body"
created_code="$(curl -sS -o "$created_body" -b "$owner_jar" -w '%{http_code}' \
  -H 'content-type: application/json' -d '{"name":"console device"}' \
  "${web}/api/cairn/devices/register")"
if [ "$created_code" != "200" ]; then
  printf '%s\n' "console device create failed: ${created_code}" >&2
  exit 1
fi
listed_body="$workdir/device-listed.body"
curl -fsS -o "$listed_body" -b "$owner_jar" "${web}/api/cairn/devices"
DEVICE_CREATED="$created_body" DEVICE_LISTED="$listed_body" python3 - <<'PY'
import json
import os
created = json.load(open(os.environ["DEVICE_CREATED"], encoding="utf-8"))
raw = open(os.environ["DEVICE_LISTED"], encoding="utf-8").read()
listed = json.loads(raw)
assert created["token"] and created["device"]["name"] == "console device"
assert created["token"] not in raw, "list leaked a token"
assert "console device" in {item["name"] for item in listed}
assert "token" not in json.dumps(listed)
device_id = created["device"]["device_id"]
open(os.environ["DEVICE_CREATED"] + ".id", "w").write(device_id)
PY
revoke_code="$(curl -sS -o /dev/null -b "$owner_jar" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d "{\"deviceId\":\"$(cat "$created_body.id")\"}" \
  "${web}/api/cairn/devices/revoke")"
if [ "$revoke_code" != "200" ]; then
  printf '%s\n' "console device revoke failed: ${revoke_code}" >&2
  exit 1
fi

for report in diagnose "projects/${project_id}/conflicts" "projects/${project_id}/freshness" \
  "projects/${project_id}/revocations" \
  "projects/${project_id}/share-candidates?email=member@example.test"; do
  report_code="$(curl -sS -o "$workdir/report.body" -b "$owner_jar" -w '%{http_code}' \
    "${web}/api/cairn/${report}")"
  if [ "$report_code" != "200" ]; then
    printf '%s\n' "report ${report} failed: ${report_code}" >&2
    cat "$workdir/report.body" >&2
    exit 1
  fi
done
bad_headers="$workdir/bad.headers"
bad_body="$workdir/bad.body"
bad_code="$(curl -sS -o "$bad_body" -D "$bad_headers" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d '{"email":"owner@example.test","password":"wrong password"}' \
  "${web}/api/cairn/auth/login")"
if [ "$bad_code" != "401" ]; then
  printf '%s\n' "invalid login status: ${bad_code}" >&2
  exit 1
fi
BAD_HEADERS="$bad_headers" BAD_BODY="$bad_body" python3 - <<'PY'
import json, os
body = json.load(open(os.environ["BAD_BODY"], encoding="utf-8"))
headers = open(os.environ["BAD_HEADERS"], encoding="utf-8").read().lower()
if body.get("error") != "invalid account credentials":
    raise SystemExit("invalid login did not keep the credential error")
if "cairn_token=" in headers:
    raise SystemExit("invalid login set a session cookie")
PY

kept_jar="$workdir/owner-kept.cookies"
cp "$owner_jar" "$kept_jar"
logout_headers="$workdir/logout.headers"
logout_body="$workdir/logout.body"
logout_code="$(curl -sS -o "$logout_body" -D "$logout_headers" -b "$owner_jar" -c "$owner_jar" -w '%{http_code}' \
  -X POST "${web}/api/cairn/auth/logout")"
if [ "$logout_code" != "204" ]; then
  printf '%s\n' "logout status: ${logout_code}" >&2
  exit 1
fi
LOGOUT_HEADERS="$logout_headers" LOGOUT_BODY="$logout_body" python3 - <<'PY'
import os, re
body = open(os.environ["LOGOUT_BODY"], encoding="utf-8").read()
headers = open(os.environ["LOGOUT_HEADERS"], encoding="utf-8").read()
if body != "":
    raise SystemExit("logout response carried a body")
match = re.search(r"(?i)cairn_token=([^;\r\n]*)", headers)
if match is None or match.group(1) != "":
    raise SystemExit("logout did not clear cairn_token")
lower = headers.lower()
if "httponly" not in lower or "max-age=0" not in lower:
    raise SystemExit("logout cookie was not an httpOnly removal")
if "content-type:" in lower:
    raise SystemExit("logout 204 set a content type")
PY

revoked_body="$workdir/revoked.body"
revoked_code="$(curl -sS -o "$revoked_body" -b "$kept_jar" -w '%{http_code}' \
  "${web}/api/cairn/projects")"
if [ "$revoked_code" != "401" ]; then
  printf '%s\n' "revoked session status: ${revoked_code}" >&2
  exit 1
fi
REVOKED_BODY="$revoked_body" python3 - <<'PY'
import json, os
body = json.load(open(os.environ["REVOKED_BODY"], encoding="utf-8"))
if body.get("error") != "account session is expired or revoked":
    raise SystemExit("revoked session error was rewritten")
PY

anon_body="$workdir/anon.body"
anon_code="$(curl -sS -o "$anon_body" -b "$owner_jar" -w '%{http_code}' \
  "${web}/api/cairn/projects")"
if [ "$anon_code" != "401" ]; then
  printf '%s\n' "signed-out project list status: ${anon_code}" >&2
  exit 1
fi
ANON_BODY="$anon_body" python3 - <<'PY'
import json, os
body = json.load(open(os.environ["ANON_BODY"], encoding="utf-8"))
if body.get("error") != "Bearer token is required":
    raise SystemExit("missing session error was rewritten")
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
