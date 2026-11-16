#!/usr/bin/env bash
# Runs every ignored PostgreSQL test in its own fresh database; each test expects an empty
# database in CAIRN_TEST_DATABASE_URL. Needs psql and a server reachable at
# CAIRN_TEST_SERVER_URL (for example postgres://user:password@127.0.0.1:5432), no database name.
set -euo pipefail

: "${CAIRN_TEST_SERVER_URL:?set CAIRN_TEST_SERVER_URL to postgres://user:password@host:port}"

n=0
failed=0
current_db=""
cleanup() {
  if [ -n "$current_db" ]; then psql "$CAIRN_TEST_SERVER_URL/postgres" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS $current_db WITH (FORCE)"; fi
}
trap cleanup EXIT
for target in "cairn-storage --test postgres" "cairn-client --test agent_flow" "cairn-server --test project_model"; do
  read -ra cargo_target <<<"$target"
  cargo test -p "${cargo_target[@]}" --no-run
  names="$(cargo test -p "${cargo_target[@]}" -- --ignored --list | sed -n 's/: test$//p')"
  if [ -z "$names" ]; then
    echo "no ignored tests listed for: $target" >&2
    exit 1
  fi
  for name in $names; do
    n=$((n + 1))
    db="cairn_ci_$$_$n"
    psql "$CAIRN_TEST_SERVER_URL/postgres" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE $db"
    current_db="$db"
    if CAIRN_TEST_DATABASE_URL="$CAIRN_TEST_SERVER_URL/$db" \
      cargo test -p "${cargo_target[@]}" -- --ignored --exact "$name" --nocapture; then
      echo "ok: $name"
    else
      echo "FAILED: $name" >&2
      failed=$((failed + 1))
    fi
    cleanup
    current_db=""
  done
done
echo "$n PostgreSQL tests, $failed failed"
[ "$failed" -eq 0 ]
