#!/bin/sh
# Runs every ignored PostgreSQL test in its own fresh database; each test expects an empty
# database in CAIRN_TEST_DATABASE_URL. Needs psql and a server reachable at
# CAIRN_TEST_SERVER_URL (for example postgres://user:password@127.0.0.1:5432), no database name.
set -eu

: "${CAIRN_TEST_SERVER_URL:?set CAIRN_TEST_SERVER_URL to postgres://user:password@host:port}"

n=0
failed=0
for target in "cairn-storage --test postgres" "cairn-client --test agent_flow"; do
  # shellcheck disable=SC2086
  cargo test -p $target --no-run
  # shellcheck disable=SC2086
  names="$(cargo test -p $target -- --ignored --list 2>/dev/null | sed -n 's/: test$//p')"
  for name in $names; do
    n=$((n + 1))
    db="cairn_ci_$n"
    psql "$CAIRN_TEST_SERVER_URL/postgres" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE $db"
    # shellcheck disable=SC2086
    if CAIRN_TEST_DATABASE_URL="$CAIRN_TEST_SERVER_URL/$db" \
      cargo test -p $target -- --ignored --exact "$name" --nocapture; then
      echo "ok: $name"
    else
      echo "FAILED: $name" >&2
      failed=$((failed + 1))
    fi
  done
done
echo "$n PostgreSQL tests, $failed failed"
[ "$n" -gt 0 ] && [ "$failed" -eq 0 ]
