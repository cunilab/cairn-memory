#!/bin/sh
set -eu

store="$(mktemp "${TMPDIR:-/tmp}/cairn-export-store.XXXXXX")"
restored="$(mktemp "${TMPDIR:-/tmp}/cairn-export-restored.XXXXXX")"
bundle="$(mktemp "${TMPDIR:-/tmp}/cairn-export-bundle.XXXXXX")"
output="$(mktemp "${TMPDIR:-/tmp}/cairn-export-output.XXXXXX")"
trap 'rm -f "$store" "$restored" "$bundle" "$output"' EXIT INT TERM
rm -f "$store" "$restored" "$bundle" "$output"

cargo run -q -p cairn-core --bin cairn -- init "$store" >/dev/null
cargo run -q -p cairn-core --bin cairn -- start "$store" export-smoke >/dev/null
cargo run -q -p cairn-core --bin cairn -- capture "$store" export-smoke export-event \
  'The export smoke test preserves redacted session evidence.' >/dev/null
cargo run -q -p cairn-core --bin cairn -- end "$store" export-smoke >/dev/null
cargo run -q -p cairn-core --bin cairn -- forget "$store" export-event
cargo run -q -p cairn-core --bin cairn -- export "$store" "$bundle"
cargo run -q -p cairn-core --bin cairn -- import "$restored" "$bundle" >"$output"

OUTPUT_JSON="$(cat "$output")" python3 - <<'PY'
import json
import os

result = json.loads(os.environ["OUTPUT_JSON"])
assert result["imported_sessions"] == 1
assert result["imported_observations"] == 1
assert result["imported_memories"] == 1
assert result["imported_forgotten_ids"] == 1
PY

context="$(cargo run -q -p cairn-core --bin cairn -- context "$restored" 'export smoke' 5)"
CONTEXT_JSON="$context" python3 - <<'PY'
import json
import os

payload = json.loads(os.environ["CONTEXT_JSON"])
assert payload["results"] == []
assert payload["omitted_count"] == 0
PY

echo "Export/import smoke check passed"
