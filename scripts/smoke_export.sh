#!/bin/sh
set -eu

. "$(dirname "$0")/smoke_agent_lib.sh"

start_server 3135
create_agent export export@example.test bootstrap
use_agent export
bundle="$smoke_dir/bundle.json"
output="$smoke_dir/import-output.json"

cairn start export-smoke >/dev/null
cairn capture export-smoke export-event \
  'The export smoke test preserves redacted session evidence.' >/dev/null
cairn end export-smoke >/dev/null
cairn forget export-event
cairn export "$bundle"

# An account import only accepts its own person's rows, so the bundle goes back into the same
# account: the import must be idempotent and must keep the forgotten evidence out of recall.
cairn import "$bundle" >"$output"

OUTPUT_JSON="$(cat "$output")" BUNDLE="$bundle" python3 - <<'PY'
import json
import os

result = json.loads(os.environ["OUTPUT_JSON"])
assert result["imported_sessions"] == 0, result
assert result["imported_observations"] == 0, result
assert result["imported_memories"] == 0, result
bundle = json.load(open(os.environ["BUNDLE"]))
assert len(bundle["memories"]) == 1 and len(bundle["observations"]) == 1
assert "export-event" in bundle["forgottenIds"]
PY

context="$(cairn context 'export smoke' 5)"
CONTEXT_JSON="$context" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["CONTEXT_JSON"])
assert payload["results"] == []
assert payload["omitted_count"] == 0
PY

echo "Export/import smoke check passed"
