#!/bin/sh
set -eu
. "$(dirname "$0")/smoke_agent_lib.sh"
start_server 3132
create_agent owner owner@example.test bootstrap
use_agent owner
project="$(cairn project-register cairn-demo 'Cairn Demo' main)"
project_id="$(printf '%s' "$project" | json_field '["project_id"]')"
record="$(PROJECT_ID="$project_id" python3 - <<'PY'
import json,os
print(json.dumps(dict(scope='project',kind='decision',title='Cache decision',summary='Use bounded writes',reason='Regression passed',details='Verified persistence',decision_context='Concurrent capture',alternatives=['unbounded writes'],consequences='Short transactions',evidence=['cargo test'],context=dict(project_id=os.environ['PROJECT_ID'],repository='cairn-demo'))))
PY
)"
saved="$(cairn save "$record")"
memory_id="$(printf '%s' "$saved" | json_field '["memory_id"]')"
cairn project-verify "$project_id" "$memory_id" abc1234 >/dev/null
cairn memory "$memory_id" | python3 -c 'import json,sys; m=json.load(sys.stdin); assert m["title"]=="Cache decision" and m["reason"]=="Regression passed" and m["alternatives"]==["unbounded writes"]'
cairn project-memories "$project_id" | python3 -c 'import json,sys; m=json.load(sys.stdin); assert len(m)==1 and m[0]["scope"]=="project"'
corrected="$(cairn correct "$memory_id" 'The cache decision stays project-scoped after correction.')"
CORRECTED="$corrected" ID="$memory_id" python3 - <<'PY'
import json,os
m=json.loads(os.environ['CORRECTED'])
assert m['scope']=='project' and m['supersedes']==os.environ['ID']
assert m['title']=='Cache decision' and m['alternatives']==['unbounded writes']
PY
cairn project-freshness "$project_id" | python3 -c 'import json,sys; assert any(e["action"]=="verification" for e in json.load(sys.stdin))'
echo 'Layer 4 scoped record CLI smoke check passed'
