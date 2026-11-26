#!/bin/sh
# Native MCP CLI stand-in (installed as `codex` and `opencode`) for hermetic setup tests.
# It keeps one `cairn` entry in $HOME/.<agent>-fixture-state and never starts a server.
PATH=/usr/bin:/bin
agent=${0##*/}
state="$HOME/.$agent-fixture-state"
[ "$1" = mcp ] || exit 2
action=$2
if [ "$agent" = codex ]; then
  [ -d "${CODEX_HOME:-$HOME/.codex}" ] || { echo "no codex home" >&2; exit 9; }
  config="${CODEX_HOME:-$HOME/.codex}/config.toml"
fi
if [ "$action" = list ]; then
  [ "$agent" = codex ] && [ "$3" = --json ] || exit 2
  if [ -f "$state" ]; then printf '[{"name":"cairn","transport":%s}]\n' "$(cat "$state")"; else echo '[]'; fi
  exit 0
fi
[ "$3" = cairn ] || exit 2
if [ "$agent" = codex ] && [ -f "$config" ]; then
  awk '/^\[/ { managed = /^\[mcp_servers\.cairn[.\]]/ } !managed' "$config" > "$config.tmp"
  mv "$config.tmp" "$config"
fi
if [ "$action" = remove ]; then rm -f "$state"; exit 0; fi
[ "$action" = add ] || exit 2
shift 3
env=
while [ "$1" != -- ]; do
  case $1 in
    --global) ;;
    --env) key=${2%%=*}; value=${2#*=}; env="$env${env:+,}\"$key\":\"$value\""; shift ;;
    *) exit 2 ;;
  esac
  shift
done
shift
printf '{"type":"stdio","command":"%s","args":["%s"],"env":{%s}}\n' "$1" "$2" "$env" > "$state"
if [ "$agent" = codex ]; then
  printf '\n[mcp_servers.cairn]\ncommand = "%s"\nargs = ["%s"]\nenv = { CAIRN_SETUP_MANAGED = "1" }\n' "$1" "$2" >> "$config"
fi
if [ "$agent" = opencode ]; then
  dir="${XDG_CONFIG_HOME:-$HOME/.config}/opencode"
  if [ ! -e "$dir/opencode.json" ] && [ ! -e "$dir/opencode.jsonc" ]; then
    mkdir -p "$dir"
    printf '{"mcp":{"servers":{"cairn":{"type":"local","command":["%s","%s"],"environment":{%s}}}}}\n' "$1" "$2" "$env" > "$dir/opencode.json"
  fi
fi
