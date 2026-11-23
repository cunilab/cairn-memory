#!/bin/sh
set -eu

unset CAIRN_URL CAIRN_DEVICE_TOKEN CAIRN_CONFIG CAIRN_OUTBOX

if [ -z "${CAIRN_TEST_DATABASE_URL:-}" ]; then
  printf '%s\n' "CAIRN_TEST_DATABASE_URL must name a PostgreSQL database with pgvector" >&2
  exit 1
fi

cd "$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"

workdir="$(mktemp -d "${TMPDIR:-/tmp}/cairn-web.XXXXXX")"
schema="smoke_web_$$"
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
import { isNoBodyStatus, isSessionRequired, upstreamPath } from "./apps/web/lib/proxy.ts";
import { signInFailureMessage } from "./apps/web/lib/session.ts";

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
if (!isSessionRequired(401, "session_required")) {
  throw new Error("expected sign-in redirect for session_required");
}
for (const [status, code] of [[401, "invalid_credentials"], [401, "unauthorized"], [403, "forbidden"], [500, "session_required"]]) {
  if (isSessionRequired(status, code)) {
    throw new Error(`${status} ${code} must not redirect to sign-in`);
  }
}
if (signInFailureMessage("invalid_credentials", "invalid account credentials") !== "Email or password is incorrect.") {
  throw new Error("credential failure copy");
}
if (signInFailureMessage("validation", "email is required") !== "email is required") {
  throw new Error("unrelated auth errors must pass through");
}
if (upstreamPath(["memories", ".."]) !== null || upstreamPath(["delete"]) !== null) {
  throw new Error("the proxy allowlist is open");
}
JS

node --experimental-strip-types --disable-warning=MODULE_TYPELESS_PACKAGE_JSON --input-type=module <<'JS'
import { forgetMemoryImpact, rotateDeviceImpact } from "./apps/web/lib/action-copy.ts";

const forgotten = forgetMemoryImpact("memory-1");
if (!forgotten.includes("memory-1") || !forgotten.includes("no longer see it") || !forgotten.includes("cannot be undone")) {
  throw new Error("forget copy must name the memory and say it cannot be undone");
}
if (/permanently|purged|undo button/i.test(forgotten)) {
  throw new Error("forget copy must not promise an undo control");
}
const rotated = rotateDeviceImpact("laptop");
if (!rotated.includes("laptop") || !rotated.includes("old token stops working") || !rotated.includes("shown once")) {
  throw new Error("rotate copy must name the device, the old token and the one-time new token");
}
const detail = await import("node:fs").then((fs) =>
  fs.readFileSync("apps/web/app/projects/_components/project-detail.tsx", "utf8"),
);
if (!detail.includes("do not repeat the action")) {
  throw new Error("a failed reload must tell the user not to repeat the action");
}
for (const removed of ["Share decision", "Revoke share", "Create team", "Promote to decision"]) {
  if (detail.includes(removed)) {
    throw new Error(`retired project action is still in the console: ${removed}`);
  }
}

JS

# The inline theme script sets the dark class before paint: stored choice first, OS preference for System.
node --experimental-strip-types --disable-warning=MODULE_TYPELESS_PACKAGE_JSON --input-type=module <<'JS'
import { THEME_SCRIPT } from "./apps/web/lib/theme.ts";

function run(stored, osDark, blocked = false) {
  const classes = new Set();
  const listeners = {};
  const sandbox = {
    document: {
      documentElement: {
        classList: { toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)) },
      },
    },
    localStorage: {
      getItem: () => {
        if (blocked) {
          throw new Error("blocked");
        }
        return stored;
      },
    },
    matchMedia: () => ({ matches: osDark, addEventListener: () => {} }),
    addEventListener: (name, fn) => (listeners[name] = fn),
  };
  new Function(...Object.keys(sandbox), THEME_SCRIPT)(...Object.values(sandbox));
  return classes.has("dark");
}
const cases = [
  ["dark", false, false, true],
  ["light", true, false, false],
  ["system", true, false, true],
  ["system", false, false, false],
  [null, true, false, true],
  [null, false, false, false],
  [null, true, true, true],
];
for (const [stored, osDark, blocked, want] of cases) {
  if (run(stored, osDark, blocked) !== want) {
    throw new Error(`theme ${stored} os-dark=${osDark} blocked=${blocked} should be dark=${want}`);
  }
}
JS

if [ ! -d apps/web/node_modules/next ]; then
  npm install --prefix apps/web
fi

cargo run -q -p cairn-server -- --database-url "$CAIRN_TEST_DATABASE_URL" --schema "$schema" --bind "127.0.0.1:${api_port}" >"$api_log" 2>&1 &
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
  # A device token is not a console session: 401 session_required, like any other non-session bearer.
  if [ "$device_denied" != "401" ]; then
    printf '%s\n' "device token on ${web_only}: ${device_denied}" >&2
    exit 1
  fi
done

curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${owner_token}" \
  -d "{\"email\":\"member@example.test\",\"password\":\"${password}\"}" \
  "${api}/cairn-memory/auth/register" >/dev/null

project_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"repository":"cairn-demo","name":"Cairn Demo"}' \
  "${api}/cairn-memory/projects/init")"
project_id="$(printf '%s' "$project_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["project_id"])')"

# A personal note stays on the owner account; a captured session distills in the background.
private_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"kind":"note","scope":"personal","title":"Private note","summary":"private observation stays on the owner account","reason":"web smoke"}' \
  "${api}/cairn-memory/remember")"
private_id="$(printf '%s' "$private_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"

curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"event_id":"evt-project","session_id":"web-project","kind":"prompt","content":"repository cache decision is ready for the project"}' \
  "${api}/cairn-memory/observe" >/dev/null
curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d '{"session_id":"web-project"}' \
  "${api}/cairn-memory/session/end" >/dev/null

promoted_json="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${device_token}" \
  -d "{\"scope\":\"project\",\"kind\":\"decision\",\"title\":\"Cache decision\",\"summary\":\"repository cache decision is ready for the project\",\"reason\":\"Verified by web smoke\",\"context\":{\"project_id\":\"${project_id}\",\"repository\":\"cairn-demo\"}}" \
  "${api}/cairn-memory/remember")"
promoted_id="$(printf '%s' "$promoted_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"

# Membership is automatic: capturing with the project id in the context joins the project.
member_login="$(curl -fsS -H 'content-type: application/json' \
  -d "{\"email\":\"member@example.test\",\"password\":\"${password}\"}" \
  "${api}/cairn-memory/auth/login")"
member_session="$(printf '%s' "$member_login" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
member_device="$(curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${member_session}" -d '{"name":"member agent"}' \
  "${api}/cairn-memory/devices/register" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
curl -fsS -H 'content-type: application/json' \
  -H "authorization: Bearer ${member_device}" \
  -d "{\"event_id\":\"member-join\",\"session_id\":\"member-join\",\"kind\":\"prompt\",\"content\":\"joined by project file\",\"context\":{\"repository\":\"cairn-demo\",\"project_id\":\"${project_id}\"}}" \
  "${api}/cairn-memory/observe" >/dev/null


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
import re
age = re.search(r"max-age=(\d+)", headers)
if age is None or not 2591000 <= int(age.group(1)) <= 2592000:
    raise SystemExit("login cookie does not live until the server session ends")
PY

me_body="$workdir/me.body"
curl -fsS -o "$me_body" -b "$owner_jar" "${web}/api/cairn/auth/me"
ME_BODY="$me_body" python3 - <<'PY'
import json, os
assert json.load(open(os.environ["ME_BODY"], encoding="utf-8"))["is_admin"] is True, "first account must be an admin"
PY

# An admin session creates accounts without its own cookie changing or the new token returned.
register_headers="$workdir/register.headers"
register_body="$workdir/register.body"
register_code="$(curl -sS -o "$register_body" -D "$register_headers" -b "$owner_jar" -w '%{http_code}' \
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
if "set-cookie:" in headers:
    raise SystemExit("register replaced the caller's session cookie")
if '"token"' in body:
    raise SystemExit("register response contains a token field")
PY
anon_register_code="$(curl -sS -o /dev/null -D "$workdir/anon-register.headers" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d "{\"email\":\"anon-register@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/register")"
if [ "$anon_register_code" != "401" ] || grep -qi 'set-cookie:' "$workdir/anon-register.headers"; then
  printf '%s\n' "anonymous register status: ${anon_register_code}" >&2
  exit 1
fi

# Admin account list, admin password reset (204, signs the user out) and self-service password change.
accounts_body="$workdir/accounts.body"
curl -fsS -o "$accounts_body" -b "$owner_jar" "${web}/api/cairn/auth/accounts"
ACCOUNTS_BODY="$accounts_body" python3 - <<'PY'
import json, os
rows = json.load(open(os.environ["ACCOUNTS_BODY"], encoding="utf-8"))
by_email = {row["email"]: row for row in rows}
for row in rows:
    assert {"person_id", "email", "is_admin", "created_at"} <= set(row), row
assert by_email["owner@example.test"]["is_admin"] is True
assert by_email["member@example.test"]["is_admin"] is False
assert "proxy-register@example.test" in by_email
open(os.environ["ACCOUNTS_BODY"] + ".id", "w").write(by_email["proxy-register@example.test"]["person_id"])
PY
reset_id="$(cat "$accounts_body.id")"
reset_password="reset horse battery staple"
reg_jar="$workdir/proxy-register.cookies"
curl -fsS -o /dev/null -c "$reg_jar" -H 'content-type: application/json' \
  -d "{\"email\":\"proxy-register@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/login"
reset_code="$(curl -sS -o "$workdir/reset.body" -b "$owner_jar" -w '%{http_code}' \
  -H 'content-type: application/json' -d "{\"password\":\"${reset_password}\"}" \
  "${web}/api/cairn/auth/accounts/${reset_id}/password")"
if [ "$reset_code" != "204" ] || [ -s "$workdir/reset.body" ]; then
  printf '%s\n' "admin password reset status: ${reset_code}" >&2
  exit 1
fi
signed_out_code="$(curl -sS -o /dev/null -b "$reg_jar" -w '%{http_code}' "${web}/api/cairn/projects")"
old_login_code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'content-type: application/json' \
  -d "{\"email\":\"proxy-register@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/login")"
if [ "$signed_out_code" != "401" ] || [ "$old_login_code" != "401" ]; then
  printf '%s\n' "reset must sign the user out and drop the old password: ${signed_out_code} ${old_login_code}" >&2
  exit 1
fi
curl -fsS -o /dev/null -c "$reg_jar" -H 'content-type: application/json' \
  -d "{\"email\":\"proxy-register@example.test\",\"password\":\"${reset_password}\"}" \
  "${web}/api/cairn/auth/login"
wrong_current_code="$(curl -sS -o /dev/null -b "$reg_jar" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d '{"current_password":"not the password","new_password":"changed horse battery staple"}' \
  "${web}/api/cairn/auth/password")"
if [ "$wrong_current_code" != "401" ]; then
  printf '%s\n' "wrong current password status: ${wrong_current_code}" >&2
  exit 1
fi
change_code="$(curl -sS -o "$workdir/change.body" -b "$reg_jar" -w '%{http_code}' \
  -H 'content-type: application/json' \
  -d "{\"current_password\":\"${reset_password}\",\"new_password\":\"changed horse battery staple\"}" \
  "${web}/api/cairn/auth/password")"
if [ "$change_code" != "204" ] || [ -s "$workdir/change.body" ]; then
  printf '%s\n' "password change status: ${change_code}" >&2
  exit 1
fi
curl -fsS -o /dev/null -H 'content-type: application/json' \
  -d '{"email":"proxy-register@example.test","password":"changed horse battery staple"}' \
  "${web}/api/cairn/auth/login"
member_accounts_jar="$workdir/member-accounts.cookies"
curl -fsS -o /dev/null -c "$member_accounts_jar" -H 'content-type: application/json' \
  -d "{\"email\":\"member@example.test\",\"password\":\"${password}\"}" \
  "${web}/api/cairn/auth/login"
member_accounts_code="$(curl -sS -o /dev/null -b "$member_accounts_jar" -w '%{http_code}' "${web}/api/cairn/auth/accounts")"
member_jobs_code="$(curl -sS -o /dev/null -b "$member_accounts_jar" -w '%{http_code}' "${web}/api/cairn/admin/jobs")"
if [ "$member_jobs_code" != "403" ]; then
  printf '%s\n' "a non-admin must not list jobs: ${member_jobs_code}" >&2
  exit 1
fi
if [ "$member_accounts_code" != "403" ]; then
  printf '%s\n' "a non-admin must not list accounts: ${member_accounts_code}" >&2
  exit 1
fi

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

# The member list the Team & sharing tab reads: owner first, then members who joined by capturing.
members_body="$workdir/members.body"
curl -fsS -o "$members_body" -b "$owner_jar" "${web}/api/cairn/projects/${project_id}/members"
MEMBERS_BODY="$members_body" python3 - <<'PY'
import json, os
rows = json.load(open(os.environ["MEMBERS_BODY"], encoding="utf-8"))
assert [row["role"] for row in rows] == ["owner", "member"], rows
assert rows[0]["email"] == "owner@example.test" and rows[1]["email"] == "member@example.test", rows
assert all({"person_id", "email", "role", "joined_at"} <= set(row) for row in rows)
PY

devices_page="$(curl -sS -o /dev/null -w '%{http_code}' -b "$owner_jar" "${web}/settings")"
if [ "$devices_page" != "200" ]; then
  printf '%s\n' "settings page status: ${devices_page}" >&2
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
assert all("health" in item for item in listed), "devices carry their health"
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

rotate_json="$(curl -fsS -b "$owner_jar" -H 'content-type: application/json' \
  -d '{"name":"rotating device"}' "${web}/api/cairn/devices/register")"
rotate_id="$(printf '%s' "$rotate_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["device"]["device_id"])')"
old_token="$(printf '%s' "$rotate_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
rotated_json="$(curl -fsS -b "$owner_jar" -H 'content-type: application/json' \
  -d "{\"deviceId\":\"${rotate_id}\"}" "${web}/api/cairn/devices/rotate")"
new_token="$(printf '%s' "$rotated_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
if [ -z "$new_token" ] || [ "$new_token" = "$old_token" ]; then
  printf '%s\n' "rotate did not issue a new token" >&2
  exit 1
fi
old_code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'content-type: application/json' \
  -H "authorization: Bearer ${old_token}" -d '{"session_id":"smoke","context":{}}' \
  "${api}/cairn-memory/session/brief")"
new_code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'content-type: application/json' \
  -H "authorization: Bearer ${new_token}" -d '{"session_id":"smoke","context":{}}' \
  "${api}/cairn-memory/session/brief")"
if [ "$old_code" != "401" ] || [ "$new_code" != "200" ]; then
  printf '%s\n' "rotated token status old=${old_code} new=${new_code}" >&2
  exit 1
fi

# Owner controls through the web session: saving a project memory, and the read-only branch list
# derived from the branch each memory was saved on.
# `remember` is an agent route the console proxy does not forward, so it goes to the API with the
# owner's session token; everything else goes through the console proxy.
owner_token="$(awk '$6 == "cairn_token" { print $7 }' "$owner_jar")"
owner_ok() {
  if [ "$2" = remember ]; then
    owner_code="$(curl -sS -o "$workdir/owner-post.body" -w '%{http_code}' -H "authorization: Bearer ${owner_token}" \
      -H 'content-type: application/json' -d "$3" "${api}/cairn-memory/remember")"
  else
    owner_code="$(curl -sS -o "$workdir/owner-post.body" -b "$owner_jar" -w '%{http_code}' \
      -H 'content-type: application/json' -d "$3" "${web}/api/cairn/$2")"
  fi
  if [ "$owner_code" != "200" ]; then
    printf '%s\n' "$1 failed: ${owner_code}" >&2
    cat "$workdir/owner-post.body" >&2
    exit 1
  fi
}
owner_ok "save project memory" remember "{\"kind\":\"decision\",\"scope\":\"project\",\"title\":\"Console decision\",\"summary\":\"Decision saved from the web console\",\"reason\":\"Web smoke verification\",\"context\":{\"project_id\":\"${project_id}\",\"repository\":\"cairn-demo\",\"branch\":\"feature-merge\"}}"
console_promoted="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$workdir/owner-post.body")"
branches_body="$workdir/branches.body"
curl -fsS -o "$branches_body" -b "$owner_jar" "${web}/api/cairn/projects/${project_id}/branches"
BRANCHES="$branches_body" python3 - <<'PY'
import json, os
rows = json.load(open(os.environ["BRANCHES"]))
assert [row["branch"] for row in rows] == ["feature-merge"], rows
assert rows[0]["memories"] == 1, rows
PY
sessions_body="$workdir/sessions.body"
curl -fsS -o "$sessions_body" -b "$owner_jar" "${web}/api/cairn/projects/${project_id}/sessions?days=30"
SESSIONS="$sessions_body" python3 - <<'PY'
import json, os
rows = json.load(open(os.environ["SESSIONS"]))
assert isinstance(rows, list), rows
PY
(
  cd apps/web
  node <<'JS'
const ts = require("typescript");
const fs = require("fs");
const path = require("path");
const Module = require("module");
const resolve = Module._resolveFilename;
Module._resolveFilename = function (request, parent, ...args) {
  return resolve.call(this, request.startsWith("@/") ? path.join(process.cwd(), request.slice(2)) : request, parent, ...args);
};
for (const ext of [".ts", ".tsx"]) {
  require.extensions[ext] = (mod, file) => mod._compile(ts.transpileModule(fs.readFileSync(file, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true },
  }).outputText, file);
}
const React = require("react");
const { renderToStaticMarkup } = require("react-dom/server");
const { ProjectSessions } = require("./app/projects/_components/project-sessions.tsx");
const session = { session_id: "s1", started_at: 1720000000, ended_at: null, branch: "main", automated: false,
  distilled_at: null, notes_written: null, digest_written: null, handoff_outcome: null, why_none: null,
  files_changed: null, captures: 1, agent_saves: 2 };
const html = renderToStaticMarkup(React.createElement(ProjectSessions, { sessions: [session] }));
for (const expected of ["project sessions", "1 sessions, 1 with saves", "main", "before 0.1.9"]) {
  if (!html.includes(expected)) throw new Error(`project sessions table missing ${expected}`);
}
if (!fs.readFileSync("app/projects/_components/project-detail.tsx", "utf8").includes("<ProjectSessions sessions={data.sessions}")) {
  throw new Error("project detail does not render the sessions table");
}
console.log("project sessions table rendered");
JS
)
for removed in "POST projects/verify" "POST projects/${project_id}/branches" "POST correct" "POST import"; do
  removed_code="$(curl -sS -o /dev/null -w '%{http_code}' -X "${removed%% *}" -b "$owner_jar" \
    -H 'content-type: application/json' -d '{}' "${web}/api/cairn/${removed#* }")"
  case "$removed_code" in
    400 | 404 | 405) ;;
    *)
      printf '%s\n' "removed route ${removed} answered ${removed_code}" >&2
      exit 1
      ;;
  esac
done
# The proxy forwards only allowlisted first segments; the retired routes never reach the server.
for refused in "POST delete" "POST export" "GET diagnose" "GET livez"; do
  refused_code="$(curl -sS -o "$workdir/refused.body" -w '%{http_code}' -X "${refused%% *}" -b "$owner_jar" \
    -H 'content-type: application/json' -d '{}' "${web}/api/cairn/${refused#* }")"
  if [ "$refused_code" != "400" ] || ! grep -q path_not_allowed "$workdir/refused.body"; then
    printf '%s\n' "route ${refused} was not refused by the proxy: ${refused_code}" >&2
    exit 1
  fi
done
dotdot_code="$(curl -sS -o /dev/null --path-as-is -b "$owner_jar" -w '%{http_code}' "${web}/api/cairn/memories/%2e%2e/%2e%2e/livez")"
if [ "$dotdot_code" = "200" ]; then
  printf '%s\n' "a dot segment escaped the allowlist" >&2
  exit 1
fi
for page in dashboard memories activity graph projects settings "projects/${project_id}"; do
  page_code="$(curl -sS -o /dev/null -w '%{http_code}' -b "$owner_jar" "${web}/${page}")"
  if [ "$page_code" != "200" ]; then
    printf '%s\n' "${page} page status: ${page_code}" >&2
    exit 1
  fi
done

if ! curl -fsS "${web}/" | grep -q 'cairn-theme'; then
  printf '%s\n' "the page does not carry the theme script" >&2
  exit 1
fi

for report in "stats?tz=420&days=30" "projects/${project_id}/conflicts" graph "auth/audit?actor=person&limit=5" "auth/audit?actor=system&type=memory_merged,links_added"; do
  report_code="$(curl -sS -o "$workdir/report.body" -b "$owner_jar" -w '%{http_code}' \
    "${web}/api/cairn/${report}")"
  if [ "$report_code" != "200" ]; then
    printf '%s\n' "report ${report} failed: ${report_code}" >&2
    cat "$workdir/report.body" >&2
    exit 1
  fi
done
curl -fsS -o "$workdir/stats.body" -b "$owner_jar" "${web}/api/cairn/stats?tz=420&days=7"
curl -fsS -o "$workdir/export.body" -b "$owner_jar" "${web}/api/cairn/memories/export"
curl -fsS -o "$workdir/dry.body" -b "$owner_jar" -H 'content-type: application/json' \
  --data-binary "@$workdir/export.body" "${web}/api/cairn/memories/import?dry_run=1"
STATS="$workdir/stats.body" EXPORT="$workdir/export.body" DRY="$workdir/dry.body" python3 - <<'PY'
import json, os
stats = json.load(open(os.environ["STATS"], encoding="utf-8"))
for key in ("usage_days", "bench", "memory_health", "devices_attention", "first_run", "capture", "jobs"):
    assert key in stats, key
assert len(stats["usage_days"]["days"]) == 7, stats["usage_days"]
export = json.load(open(os.environ["EXPORT"], encoding="utf-8"))
assert export["version"] == 1 and isinstance(export["memories"], list), export
dry = json.load(open(os.environ["DRY"], encoding="utf-8"))
assert dry["new"] == 0 and dry["invalid"] == [], dry  # everything exported already exists
assert dry["duplicate"] == len(export["memories"]), dry
PY
# Forget through the web: the same erase the memory sheet runs.
owner_ok "save a memory to forget" remember '{"kind":"note","scope":"personal","title":"Forget me from the console","summary":"A throwaway note for the forget smoke.","reason":"Web smoke verification"}'
forget_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$workdir/owner-post.body")"
owner_ok "forget a memory" forget "{\"id\":\"${forget_id}\"}"
forgotten_code="$(curl -sS -o /dev/null -b "$owner_jar" -w '%{http_code}' "${web}/api/cairn/memories/${forget_id}")"
if [ "$forgotten_code" != "404" ]; then
  printf '%s\n' "a forgotten memory still answers: ${forgotten_code}" >&2
  exit 1
fi
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
if body.get("error") != "invalid account credentials" or body.get("code") != "invalid_credentials":
    raise SystemExit("invalid login did not keep the credential error and code")
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
if body.get("error") != "account session is expired or revoked" or body.get("code") != "session_required":
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
if body.get("error") != "Bearer token is required" or body.get("code") != "session_required":
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
  "${web}/api/cairn/projects/${project_id}/memories")"
if [ "$shared_code" != "200" ]; then
  printf '%s\n' "member shared read failed: ${shared_code}" >&2
  cat "$shared_body" >&2
  exit 1
fi
SHARED_BODY="$shared_body" PROMOTED_ID="$promoted_id" PRIVATE_ID="$private_id" python3 - <<'PY'
import json
import os
memories = json.load(open(os.environ["SHARED_BODY"], encoding="utf-8"))
ids = [memory["id"] for memory in memories]
assert os.environ["PROMOTED_ID"] in ids
assert os.environ["PRIVATE_ID"] not in ids
blob = json.dumps(memories)
assert "private observation stays on the owner account" not in blob
PY

echo "Web console smoke check passed"
