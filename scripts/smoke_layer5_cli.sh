#!/bin/sh
set -eu
. "$(dirname "$0")/smoke_agent_lib.sh"
as_member() { XDG_CONFIG_HOME="$smoke_dir/member" "$@"; }
as_reader() { XDG_CONFIG_HOME="$smoke_dir/reader" "$@"; }
start_server 3133
create_agent owner owner@example.test bootstrap
create_agent member member@example.test register
create_agent reader reader@example.test register
use_agent owner
checkout="$smoke_dir/checkout"
git init -q "$checkout"
git -C "$checkout" remote add origin git@EXAMPLE.TEST:acme/cairn-demo.git
(cd "$checkout" && cairn init >/dev/null)
project_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["project_id"])' "$checkout/.cairn/project.json")"
(cd "$checkout" && as_member cairn init) | grep -q 'already initialized, you are a member'
(cd "$checkout" && as_reader cairn capture join-reader reader-event 'reader joined' >/dev/null)
(cd "$checkout" && as_member cairn start source-session example.test/acme/cairn-demo main >/dev/null)
(cd "$checkout" && as_member cairn capture source-session source-event 'climodel shared raw capture because regression passed' >/dev/null)
as_member cairn end source-session >/dev/null
saved="$(cd "$checkout" && as_member cairn save '{"scope":"project","kind":"rule","title":"climodel rule","summary":"Bound concurrent writes","reason":"Prevent lost data","rule":"Use transactions","exceptions":"read-only"}')"
memory_id="$(printf '%s' "$saved" | json_field '["memory_id"]')"
for who in owner member reader; do
  rows="$(XDG_CONFIG_HOME="$smoke_dir/$who" cairn project-memories "$project_id")"
  ROWS="$rows" ID="$memory_id" python3 - <<'PY'
import json,os
r=json.loads(os.environ['ROWS'])
assert any(m['memory_id']==os.environ['ID'] and m['rule']=='Use transactions' for m in r)
assert any(m['kind']=='capture' and 'shared raw capture' in m['summary'] for m in r)
assert any(m['kind']=='session_summary' for m in r)
PY
done
as_member cairn lesson '{"scope":"personal","title":"climodel concise","summary":"Prefer concise answers","reason":"Save attention","kind":"preference","preference":"concise"}' >/dev/null
as_member cairn save '{"scope":"global","title":"climodel lesson","summary":"Lock shared writes","reason":"Avoid races","kind":"gotcha","cause":"concurrency","fix":"transaction"}' >/dev/null
as_reader cairn memories | python3 -c 'import json,sys; r=json.load(sys.stdin); assert any(m["scope"]=="global" for m in r); assert not any(m["scope"]=="personal" and "climodel concise" in m["title"] for m in r)'
if as_reader cairn correct "$memory_id" bad >/dev/null 2>&1; then exit 1; fi
if as_reader cairn delete-memory "$memory_id" >/dev/null 2>&1; then exit 1; fi
cairn project-verify "$project_id" "$memory_id" verified >/dev/null
for method in GET POST; do
  status="$(curl -sS -o /dev/null -w '%{http_code}' -X "$method" "$base/cairn-memory/promote" -H 'Content-Type: application/json' -H "Authorization: Bearer $(session_of owner)" -d '{}')"
  [ "$status" = 404 ] || [ "$status" = 405 ]
done
for old in promote promote-preview project-share project-team; do
  if cairn "$old" "$project_id" >/dev/null 2>&1; then exit 1; fi
done
cairn delete-memory "$memory_id" >/dev/null
as_reader cairn project-memories "$project_id" | ID="$memory_id" python3 -c 'import json,sys,os; assert not any(m["memory_id"]==os.environ["ID"] for m in json.load(sys.stdin))'
echo 'Layer 5 shared scope CLI smoke check passed'
