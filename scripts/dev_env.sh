# Source from the repo root (`source scripts/dev_env.sh`); dev_up.sh and dev_down.sh source it too.
# 1. Exports CAIRN_TEST_DATABASE_URL for `cargo test --workspace` against the dev stack's Postgres
#    (127.0.0.1:55432). It needs .env.dev, which dev_up.sh creates on first run.
# 2. Picks the compose command and works around a ~/.docker/config.json whose credential helper is
#    not installed (public images only, so no credentials are needed).
if [ -f .env.dev ]; then
  cairn_pw=$(sed -n 's/^POSTGRES_PASSWORD=//p' .env.dev)
  [ -n "$cairn_pw" ] && export CAIRN_TEST_DATABASE_URL="postgres://cairn:${cairn_pw}@127.0.0.1:55432/cairn"
  unset cairn_pw
fi

if docker compose version >/dev/null 2>&1; then dc="docker compose"; else dc="docker-compose"; fi

if grep -q '"credsStore"' "${DOCKER_CONFIG:-$HOME/.docker}/config.json" 2>/dev/null; then
  helper=$(sed -n 's/.*"credsStore"[^"]*"\([^"]*\)".*/\1/p' "${DOCKER_CONFIG:-$HOME/.docker}/config.json")
  if ! command -v "docker-credential-$helper" >/dev/null 2>&1; then
    DOCKER_HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}')
    export DOCKER_HOST DOCKER_CONFIG="${TMPDIR:-/tmp}/cairn-dev-docker"
    mkdir -p "$DOCKER_CONFIG"
    printf '{"cliPluginsExtraDirs":["%s/.docker/cli-plugins","/opt/homebrew/lib/docker/cli-plugins"]}\n' "$HOME" >"$DOCKER_CONFIG/config.json"
  fi
fi

# Cache mounts in docker/*.Dockerfile need BuildKit, which compose reaches via buildx.
docker buildx version >/dev/null 2>&1 || { echo "docker buildx plugin missing (brew install docker-buildx)" >&2; return 1 2>/dev/null || exit 1; }
