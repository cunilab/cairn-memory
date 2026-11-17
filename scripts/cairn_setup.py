"""Idempotent agent config installation, invoked by the version-matched CLI.

Only Cairn-marked entries are replaceable. Preflight all files before invoking
native MCP config commands or writing local blocks. Credentials stay in Cairn.
"""
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

MARKER = 'CAIRN_SETUP_MANAGED'
BEGIN = '<!-- cairn:begin -->'
END = '<!-- cairn:end -->'


def read(path):
    try:
        return path.read_text(encoding='utf-8')
    except FileNotFoundError:
        return ''


def block(text, begin, end, content, remove):
    if text.count(begin) != text.count(end) or text.count(begin) > 1:
        raise ValueError('malformed Cairn markers; repair the marked block first')
    if begin in text:
        start = text.index(begin)
        finish = text.index(end) + len(end)
        if finish <= start:
            raise ValueError('malformed Cairn markers')
        if text[finish:finish+1] == '\n':
            finish += 1
        text = text[:start] + text[finish:]
    if not remove:
        text += ('' if not text or text.endswith('\n') else '\n') + begin + '\n' + content.rstrip() + '\n' + end + '\n'
    return text


def json_config(path):
    raw = read(path)
    value = json.loads(raw) if raw.strip() else {}
    if not isinstance(value, dict):
        raise ValueError(f'{path.name} must contain a JSON object')
    return value


def object_at(root, *keys):
    for key in keys:
        root = root.setdefault(key, {})
        if not isinstance(root, dict):
            raise ValueError(f'{key} must be an object')
    return root


def set_mcp(root, keys, value, remove):
    entries = object_at(root, *keys)
    existing = entries.get('cairn')
    if existing is not None and not owned(existing):
        raise ValueError('an unmarked cairn MCP entry exists; rename or remove it explicitly')
    if remove:
        entries.pop('cairn', None)
    else:
        entries['cairn'] = value


def hook_config(root, agent, script, config, remove):
    hooks = object_at(root, 'hooks')
    command = shlex.join(['python3', str(script), '--agent', agent, '--config', str(config), '--cairn-managed'])
    for event in ['SessionStart', 'UserPromptSubmit', 'PostToolUse', 'Stop', 'SessionEnd']:
        groups = hooks.get(event, [])
        if not isinstance(groups, list):
            raise ValueError(f'{event} must be an array')
        kept = []
        for group in groups:
            if not isinstance(group, dict) or not isinstance(group.get('hooks'), list):
                raise ValueError(f'invalid {event} hook group')
            if any(not isinstance(h, dict) for h in group['hooks']):
                raise ValueError(f'invalid {event} hook handler')
            handlers = [h for h in group['hooks'] if '--cairn-managed' not in str(h.get('command', ''))]
            if handlers:
                kept.append(dict(group, hooks=handlers))
        if not remove:
            handler = {'type':'command', 'command':command, 'timeout':3 if event == 'SessionEnd' else 20}
            group = {'hooks':[handler]}
            if event == 'PostToolUse':
                group['matcher'] = 'Bash|Edit|Write|MultiEdit|apply_patch'
            kept.append(group)
        if kept:
            hooks[event] = kept
        else:
            hooks.pop(event, None)


class JsonNode:
    def __init__(self, value, start, end, members=None):
        self.value, self.start, self.end = value, start, end
        self.members = members or {}
        self.key_start, self.comma, self.previous_comma = start, None, None


def jsonc(text):
    # Token spans let removal leave unrelated whitespace, strings and comments untouched.
    pattern = r'"(?:\\.|[^"\\])*"|//[^\r\n]*|/\*[\s\S]*?\*/|[{}\[\],:]|[^\s{}\[\],:]+'
    tokens = []
    end = 0
    for match in re.finditer(pattern, text):
        if text[end:match.start()].strip():
            raise ValueError('invalid JSONC syntax')
        value = match.group()
        if value.startswith('/*') and not value.endswith('*/'):
            raise ValueError('unclosed JSONC comment')
        if not value.startswith(('//', '/*')):
            tokens.append((value, match.start(), match.end()))
        end = match.end()
    if text[end:].strip():
        raise ValueError('invalid JSONC syntax')
    def parse(index, depth=0):
        if index >= len(tokens) or depth > 64:
            raise ValueError('invalid or too deeply nested JSONC')
        token, start, end = tokens[index]
        if token not in ('{', '['):
            try:
                value = json.loads(token)
            except (ValueError, TypeError):
                raise ValueError('invalid JSONC value') from None
            return JsonNode(value, start, end), index + 1
        closing = '}' if token == '{' else ']'
        value, members = ({} if token == '{' else []), {}
        index += 1
        previous = None
        while index < len(tokens) and tokens[index][0] != closing:
            key_start = tokens[index][1]
            if token == '{':
                key = json.loads(tokens[index][0])
                if not isinstance(key, str) or key in value or index+1 >= len(tokens) or tokens[index+1][0] != ':':
                    raise ValueError('invalid or duplicate JSONC key')
                index += 2
            child, index = parse(index, depth+1)
            child.key_start, child.previous_comma = key_start, previous
            if token == '{':
                value[key], members[key] = child.value, child
            else:
                value.append(child.value)
            if index < len(tokens) and tokens[index][0] == ',':
                child.comma = tokens[index][1:]
                previous = child.comma
                index += 1
            elif index >= len(tokens) or tokens[index][0] != closing:
                raise ValueError('missing JSONC separator')
        if index >= len(tokens):
            raise ValueError('unclosed JSONC container')
        return JsonNode(value, start, tokens[index][2], members), index+1
    root, used = parse(0)
    if used != len(tokens) or not isinstance(root.value, dict):
        raise ValueError('config must be one JSONC object')
    return root


def owned(entry):
    return isinstance(entry, dict) and isinstance(entry.get('environment', entry.get('env')), dict) and entry.get('environment', entry.get('env')).get(MARKER) == '1'


def remove_cairn(text, root, *, allow_trailing):
    node = root
    for key in ['mcp', 'servers', 'cairn']:
        if key not in node.members:
            return text
        node = node.members[key]
    if not owned(node.value):
        raise ValueError('an unmarked cairn MCP entry exists; rename or remove it explicitly')
    cuts = [(node.key_start, node.end)]
    if node.comma:
        cuts.append(node.comma)
    elif node.previous_comma and not allow_trailing:
        cuts.append(node.previous_comma)
    for start, end in sorted(cuts, reverse=True):
        text = text[:start] + text[end:]
    return text


def native(agent, args, home):
    executable = shutil.which(agent)
    if not executable:
        raise ValueError(f'the installed {agent} CLI is required for agent setup/removal')
    if agent == 'codex':
        folder = Path(os.environ.get('CODEX_HOME') or str(home/'.codex'))
        folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    try:
        result = subprocess.run([executable, 'mcp', *args], cwd=home,
            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20, check=False)
    except (OSError, subprocess.SubprocessError):
        raise ValueError(f'{agent} MCP command could not complete') from None
    if result.returncode:
        raise ValueError(f'{agent} MCP command failed; check the native CLI configuration')
    return result.stdout


def codex_mcp(spec, env, home):
    entries = json.loads(native('codex', ['list', '--json'], home))
    if not isinstance(entries, list):
        raise ValueError('codex mcp list returned an invalid configuration')
    current = next((entry for entry in entries if entry.get('name') == 'cairn'), None)
    if current is not None and not owned(current.get('transport')):
        raise ValueError('an unmarked cairn MCP entry exists; rename or remove it explicitly')
    if spec['remove']:
        return (lambda: native('codex', ['remove', 'cairn'], home)) if current else (lambda: None)
    transport = (current or {}).get('transport', {})
    if transport.get('command') == spec['exe'] and transport.get('args') == ['mcp'] and transport.get('env') == env:
        return lambda: None
    args = ['add', 'cairn']
    for key, value in env.items(): args.extend(['--env', key+'='+value])
    args.extend(['--', spec['exe'], 'mcp'])
    return lambda: native('codex', args, home)


def configure(spec):
    agent, remove = spec['agent'], spec['remove']
    config, exe = Path(spec['config']), spec['exe']
    home = Path(os.environ.get('HOME') or os.environ['USERPROFILE'])
    xdg = Path(os.environ.get('XDG_CONFIG_HOME') or str(home/'.config'))
    env = {'XDG_CONFIG_HOME':str(config.parent.parent), MARKER:'1'}
    mcp = {'type':'stdio', 'command':exe, 'args':['mcp'], 'env':env}
    files = {}
    native_action = lambda: None
    if agent in ['claude', 'codex']:
        folder = Path(os.environ.get('CLAUDE_CONFIG_DIR' if agent == 'claude' else 'CODEX_HOME') or str(home/('.claude' if agent == 'claude' else '.codex')))
        path = folder/('settings.json' if agent == 'claude' else 'hooks.json')
        root = json_config(path)
        hook_config(root, agent, config.parent/'hook/cairn_hook.py', config, remove)
        files[path] = json.dumps(root, indent=2) + '\n'
        if agent == 'claude':
            path = folder.parent/'.claude.json' if not os.environ.get('CLAUDE_CONFIG_DIR') else folder/'.claude.json'
            root = json_config(path)
            set_mcp(root, ['mcpServers'], mcp, remove)
            files[path] = json.dumps(root, indent=2) + '\n'
        else:
            native_action = codex_mcp(spec, env, home)
    elif agent == 'opencode':
        if not shutil.which('opencode'):
            raise ValueError('the installed opencode CLI is required for agent setup/removal')
        base = xdg/'opencode'
        choices = [path for path in [base/'opencode.json', base/'opencode.jsonc'] if path.exists()]
        if len(choices) > 1:
            raise ValueError('both opencode.json and opencode.jsonc exist; choose one global config first')
        path = choices[0] if choices else base/'opencode.json'
        raw = read(path)
        root = jsonc(raw or '{}')
        mcp = root.value.get('mcp', {})
        if not isinstance(mcp, dict) or not isinstance(mcp.get('servers', {}), dict):
            raise ValueError('OpenCode V2 requires an mcp.servers object')
        if 'cairn' in mcp:
            raise ValueError('OpenCode V1 MCP config found; migrate to V2 first')
        entry = mcp.get('servers', {}).get('cairn')
        if entry is not None and not owned(entry):
            raise ValueError('an unmarked cairn MCP entry exists; rename or remove it explicitly')
        desired = {'type':'local', 'command':[exe, 'mcp'], 'environment':env}
        if remove:
            files[path] = remove_cairn(raw, root, allow_trailing=path.suffix == '.jsonc')
        elif entry != desired:
            args = ['add', 'cairn', '--global']
            for key, value in env.items(): args.extend(['--env', key+'='+value])
            args.extend(['--', exe, 'mcp'])
            native_action = lambda: native('opencode', args, home)
        path = xdg/'opencode/AGENTS.md'
        files[path] = block(read(path), BEGIN, END, spec['instructions'], remove)
    else:
        raise ValueError('agent must be claude, codex or opencode')
    native_action()
    for path, body in files.items():
        if read(path) == body:
            continue
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd, temporary = tempfile.mkstemp(prefix='.cairn-', dir=path.parent)
        try:
            with os.fdopen(fd, 'w', encoding='utf-8') as output:
                output.write(body)
            os.replace(temporary, path)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    return f'Cairn {agent} integration ' + ('removed' if remove else 'installed')


if __name__ == '__main__':
    try:
        print(configure(json.load(sys.stdin)))
    except (OSError, ValueError, KeyError, TypeError, ImportError) as error:
        sys.exit('Cairn agent setup failed: ' + str(error))
