# Sourced by dev_up.sh / dev_down.sh: pick the compose command and work around
# a ~/.docker/config.json whose credential helper is not installed (public
# images only, so no credentials are needed).
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
docker buildx version >/dev/null 2>&1 || { echo "docker buildx plugin missing (brew install docker-buildx)" >&2; exit 1; }
