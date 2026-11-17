#!/usr/bin/env python3
"""Native MCP CLI stand-in for hermetic installer tests (never starts a server)."""
import json
import os
import sys
from pathlib import Path

agent = Path(sys.argv[0]).name
assert sys.argv[1] == 'mcp'
action = sys.argv[2]
home = Path(os.environ['HOME'])
state = home/('.'+agent+'-fixture-state.json')
entries = json.loads(state.read_text()) if state.exists() else {}
if action == 'list':
    assert agent == 'codex' and sys.argv[3:] == ['--json']
    print(json.dumps([{'name':name, 'transport':value} for name, value in entries.items()]))
    sys.exit(0)
assert sys.argv[3] == 'cairn'
if action == 'add':
    tail=sys.argv[4:]; env={}
    while tail and tail[0] != '--':
        flag=tail.pop(0)
        if flag == '--global': continue
        assert flag == '--env'
        key,value=tail.pop(0).split('=',1); env[key]=value
    assert tail.pop(0) == '--'
    entries['cairn']={'type':'stdio','command':tail[0],'args':tail[1:],'env':env}
else:
    assert agent == 'codex' and action == 'remove'
    entries.pop('cairn',None)
state.write_text(json.dumps(entries))
if agent == 'codex':
    path=home/'.codex/config.toml'; path.parent.mkdir(exist_ok=True)
    text=path.read_text() if path.exists() else ''
    text=text.split('# fixture MCP')[0]
    if entries:
        text+='# fixture MCP\n[mcp_servers.cairn]\ncommand = '+json.dumps(entries['cairn']['command'])+'\n[mcp_servers.cairn.env]\nCAIRN_SETUP_MANAGED = "1"\n'
    path.write_text(text)
else:
    path=home/'.config/opencode/opencode.json'; path.parent.mkdir(parents=True,exist_ok=True)
    root=json.loads(path.read_text()) if path.exists() else {}
    item=entries['cairn']
    root.setdefault('mcp',{}).setdefault('servers',{})['cairn']={'type':'local','command':[item['command'],*item['args']],'environment':item['env']}
    path.write_text(json.dumps(root,indent=2)+'\n')
