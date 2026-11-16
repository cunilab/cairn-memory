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
[ "$(cairn project-list)" = '[]' ]
checkout="$smoke_dir/checkout"
git init -q "$checkout"
git -C "$checkout" remote add origin git@EXAMPLE.TEST:acme/cairn-demo.git
(cd "$checkout" && cairn init >/dev/null)
project_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["project_id"])' "$checkout/.cairn/project.json")"
repository=example.test/acme/cairn-demo
(cd "$checkout" && as_member cairn init) | grep -q 'already initialized, you are a member'
(cd "$checkout" && as_reader cairn capture join-reader reader-event 'reader joined' >/dev/null)
(cd "$checkout" && as_member cairn start source-session "$repository" main >/dev/null)
(cd "$checkout" && as_member cairn capture source-session source-event 'climodel shared decision because the regression passed' >/dev/null)
ended="$(as_member cairn end source-session)"
memory_id="$(printf '%s' "$ended" | json_field '["memory_id"]')"
preview="$(as_member cairn promote-preview "$project_id" "$memory_id" 'publish full payload')"
PREVIEW="$preview" python3 - <<'PY'
import json,os
p=json.loads(os.environ['PREVIEW'])
assert 'climodel shared decision' in p['content']
assert p['published_by'] and not p['source_event_ids']
PY
[ "$(as_reader cairn project-memories "$project_id")" = '[]' ]
promoted="$(as_member cairn promote "$project_id" "$memory_id" 'publish full payload')"
promoted_id="$(printf '%s' "$promoted" | json_field '["memory_id"]')"
for who in owner member reader; do
  decisions="$(XDG_CONFIG_HOME="$smoke_dir/$who" cairn project-memories "$project_id")"
  DECISIONS="$decisions" ID="$promoted_id" python3 - <<'PY'
import json,os
rows=json.loads(os.environ['DECISIONS'])
assert len(rows)==1 and rows[0]['memory_id']==os.environ['ID']
assert rows[0]['promoted_from'] is None
PY
done
as_member cairn lesson 'climodel prefer clear tests' >/dev/null
recall="$(cd "$checkout" && as_member cairn context climodel 10)"
RECALL="$recall" python3 - <<'PY'
import json,os
rows=json.loads(os.environ['RECALL'])['results']
assert {r['source'] for r in rows}=={'personal','project'}
PY
if as_reader cairn project-verify "$project_id" "$promoted_id" bad >/dev/null 2>&1; then exit 1; fi
if as_reader cairn correct "$promoted_id" bad >/dev/null 2>&1; then exit 1; fi
if as_reader cairn delete-memory "$promoted_id" >/dev/null 2>&1; then exit 1; fi
cairn project-verify "$project_id" "$promoted_id" verified >/dev/null
as_member cairn correct "$memory_id" 'climodel private source correction' >/dev/null
[ "$(as_reader cairn project-memories "$project_id" | json_field '.__len__()')" = 1 ]
for route in team shared shares policy share-candidates revocations; do
  for method in GET POST; do
    status="$(curl -sS -o /dev/null -w '%{http_code}' -X "$method" "$base/cairn-memory/projects/$project_id/$route" -H 'Content-Type: application/json' -H "Authorization: Bearer $(session_of owner)" -d '{}')"
    [ "$status" = 404 ] || { printf '%s\n' "$method $route returned $status" >&2; exit 1; }
  done
done
for old in project-share project-shared project-revoke project-team project-team-create; do
  if cairn "$old" "$project_id" >/dev/null 2>&1; then exit 1; fi
done
people="$(as_reader cairn project-members "$project_id")"
PEOPLE="$people" python3 - <<'PY'
import json,os
p=json.loads(os.environ['PEOPLE'])
assert len(p)==3 and p[0]['role']=='owner'
PY
cairn delete-memory "$promoted_id" >/dev/null
[ "$(as_reader cairn project-memories "$project_id")" = '[]' ]
as_member cairn export "$smoke_dir/personal.json"
python3 - "$smoke_dir/personal.json" <<'PY'
import json,sys
assert all(m.get('published_by') is None for m in json.load(open(sys.argv[1]))['memories'])
PY
echo 'Layer 5 shared project CLI smoke check passed'
