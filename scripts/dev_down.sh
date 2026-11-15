#!/bin/sh
# Stop the Docker dev stack; --wipe also drops the Postgres volume.
set -eu
cd "$(dirname "$0")/.."

. scripts/dev_env.sh

case "${1:-}" in
  "") flags="" ;;
  --wipe) flags="--volumes" ;;
  *) echo "usage: $0 [--wipe]" >&2; exit 2 ;;
esac

[ -f .env.dev ] || printf 'POSTGRES_PASSWORD=unused\n' >.env.dev
# shellcheck disable=SC2086
$dc -f docker-compose.dev.yml --env-file .env.dev down $flags
