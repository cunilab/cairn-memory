#!/bin/sh
# Build and start the isolated Docker dev stack (project cairn-dev).
set -eu
cd "$(dirname "$0")/.."

. scripts/dev_env.sh

if [ ! -f .env.dev ]; then
  umask 077
  printf 'POSTGRES_PASSWORD=%s\n' "$(openssl rand -hex 24)" >.env.dev
  echo "created .env.dev"
fi

$dc -f docker-compose.dev.yml --env-file .env.dev up -d --build --wait --wait-timeout 600

cat <<'URLS'
postgres  127.0.0.1:55432 (user cairn, db cairn, password in .env.dev)
server    http://127.0.0.1:58080/cairn-memory/health
web       http://127.0.0.1:53000/login
URLS
