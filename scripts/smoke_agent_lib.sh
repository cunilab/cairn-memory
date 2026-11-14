# Sourced by the agent smoke scripts (CLI and MCP). Starts a file-store server, creates accounts
# and device tokens through the web API as the web console does, and points `cairn` at it with
# a device token. The agent side never holds an account session.

repo="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$repo"
smoke_dir="$(mktemp -d "${TMPDIR:-/tmp}/cairn-agent-smoke.XXXXXX")"
server_pid=""
password="correct horse battery staple"

agent_cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$smoke_dir"
}
trap agent_cleanup EXIT INT TERM

cargo build -q -p cairn-server -p cairn-client
bin_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug"

json_field() {
  python3 -c 'import json,sys; print(eval("d" + sys.argv[1], {"d": json.load(sys.stdin)}))' "$1"
}

# start_server <port>: the server also serves the web API used for account setup.
start_server() {
  base="http://127.0.0.1:$1"
  "$bin_dir/cairn-server" --store "$smoke_dir/store.json" --bind "127.0.0.1:$1" >"$smoke_dir/server.log" 2>&1 &
  server_pid=$!
  for _ in $(seq 1 100); do
    if curl -fsS "$base/cairn-memory/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  printf '%s\n' "server did not become ready" >&2
  cat "$smoke_dir/server.log" >&2
  exit 1
}

# create_agent <name> <email> <bootstrap|register>: new account, one device token, and a CLI
# config home of its own. The web session token is kept for web-only steps (session_of).
create_agent() {
  session="$(curl -fsS -X POST "$base/cairn-memory/auth/$3" \
    -H 'Content-Type: application/json' \
    -d "{\"email\":\"$2\",\"password\":\"$password\"}" | json_field '["token"]')"
  device="$(curl -fsS -X POST "$base/cairn-memory/devices/register" \
    -H 'Content-Type: application/json' -H "Authorization: Bearer $session" \
    -d "{\"name\":\"$1\"}" | json_field '["token"]')"
  printf '%s' "$session" >"$smoke_dir/$1.session"
  XDG_CONFIG_HOME="$smoke_dir/$1" "$bin_dir/cairn" setup "$base" <<<"$device" >/dev/null
  printf '%s' "$device" >"$smoke_dir/$1.device"
}

session_of() { cat "$smoke_dir/$1.session"; }
device_of() { cat "$smoke_dir/$1.device"; }
# use_agent <name>: later `cairn` calls use that agent's config.
use_agent() { XDG_CONFIG_HOME="$smoke_dir/$1"; export XDG_CONFIG_HOME; }
cairn() { "$bin_dir/cairn" "$@"; }
