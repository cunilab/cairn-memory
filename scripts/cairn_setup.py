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
# Autoloaded from <config-home>/opencode/plugins/cairn/. A single file in the
# plugins config array is skipped by OpenCode. This file has no credential.
PLUGIN_MARKER = "// cairn:continuity\n"
PLUGIN = PLUGIN_MARKER + r'''import { spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { join } from "node:path"

const HOOK_SCRIPT = __CAIRN_HOOK_SCRIPT__
const SETUP_CONFIG = __CAIRN_SETUP_CONFIG__

function configFile() {
  const fromEnv = (process.env.CAIRN_CONFIG || "").trim()
  if (fromEnv) return fromEnv
  return SETUP_CONFIG
}

function sessionDirectory(session) {
  if (!session || typeof session !== "object") return ""
  const nested = session.data && session.data.location
  const location = session.location || nested
  if (location && typeof location.directory === "string") return location.directory
  return typeof session.directory === "string" ? session.directory : ""
}

function structuredInput(input) {
  const out = {}
  if (!input || typeof input !== "object") return out
  for (const key of ["file_path", "filePath", "path", "file", "command", "cmd"]) {
    const value = input[key]
    if (typeof value === "string" && value && value.length <= 512 && !value.includes("\n")) out[key] = value
  }
  return out
}

function runHook(payload) {
  if (!payload || typeof payload.cwd !== "string" || !payload.cwd) return Promise.resolve("")
  const config = configFile()
  if (!existsSync(HOOK_SCRIPT)) return Promise.resolve("")
  return new Promise((resolve) => {
    const child = spawn("python3", [HOOK_SCRIPT, "--agent", "opencode", "--config", config, "--cairn-managed"], { stdio: ["pipe", "pipe", "ignore"] })
    let out = ""
    const timer = setTimeout(() => { child.kill(); resolve("") }, 4000)
    child.stdout.on("data", (chunk) => { out += chunk })
    child.on("error", () => { clearTimeout(timer); resolve("") })
    child.on("close", () => {
      clearTimeout(timer)
      try {
        const parsed = JSON.parse(out)
        const text = parsed && parsed.hookSpecificOutput && parsed.hookSpecificOutput.additionalContext
        resolve(typeof text === "string" ? text : "")
      } catch (error) {
        resolve("")
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

async function directoryFor(ctx, sessionID) {
  if (!sessionID || !ctx.session || typeof ctx.session.get !== "function") return ""
  try {
    return sessionDirectory(await ctx.session.get({ sessionID }))
  } catch (error) {
    return ""
  }
}

function pushText(event, text) {
  if (!text || !event || !Array.isArray(event.system)) return
  event.system.push({ type: "text", text })
}

function ignored(directory) {
  return directory && existsSync(join(directory, ".cairnignore"))
}

export default {
  id: "cairn",
  async setup(ctx) {
    if (!ctx || !ctx.session || typeof ctx.session.hook !== "function") return
    const briefed = new Set()
    const pending = new Map()
    const queue = (sessionID, text) => {
      if (!sessionID || !text) return
      const list = pending.get(sessionID) || []
      list.push(text)
      pending.set(sessionID, list)
    }
    const take = (sessionID) => {
      const list = pending.get(sessionID) || []
      pending.delete(sessionID)
      return list.join("\n")
    }
    await ctx.session.hook("prompt", async (event) => {
      const sessionID = event && (event.sessionID || event.sessionId)
      const directory = await directoryFor(ctx, sessionID)
      const text = event && event.prompt && event.prompt.text
      if (!directory || typeof text !== "string" || !text) return
      await runHook({ hook_event_name: "UserPromptSubmit", session_id: sessionID, cwd: directory, prompt: text })
    })
    await ctx.session.hook("context", async (event) => {
      const sessionID = event && event.sessionID
      const directory = await directoryFor(ctx, sessionID)
      if (!directory || ignored(directory) || !event) return
      const parts = []
      if (!briefed.has(sessionID)) {
        briefed.add(sessionID)
        parts.push(await runHook({ hook_event_name: "SessionStart", session_id: sessionID, cwd: directory, source: "startup" }))
      }
      parts.push(take(sessionID))
      pushText(event, parts.filter(Boolean).join("\n"))
    })
    await ctx.session.hook("compaction", async (event) => {
      const sessionID = event && event.sessionID
      const directory = await directoryFor(ctx, sessionID)
      if (!directory || ignored(directory) || !event) return
      await runHook({ hook_event_name: "Stop", session_id: sessionID, cwd: directory })
      briefed.delete(sessionID)
      pushText(event, await runHook({ hook_event_name: "SessionStart", session_id: sessionID, cwd: directory, source: "compact" }))
    })
    if (ctx.tool && typeof ctx.tool.hook === "function") {
      try { await ctx.tool.hook("execute.after", async (event) => {
        const sessionID = event && (event.sessionID || event.sessionId)
        const directory = await directoryFor(ctx, sessionID)
        if (!directory || ignored(directory)) return
        const name = event && (event.tool || event.name || "")
        const input = structuredInput(event && (event.input || event.args || event.tool_input))
        const text = await runHook({ hook_event_name: "PostToolUse", session_id: sessionID, cwd: directory, tool_name: name, tool_input: input, tool_use_id: event && (event.callID || event.id) })
        queue(sessionID, text)
      }) } catch (error) {}
    }
    if (ctx.event && typeof ctx.event.subscribe === "function") {
      const controller = new AbortController()
      ;(async () => {
        for await (const item of ctx.event.subscribe({ signal: controller.signal })) {
          // 2.1.4 ends a turn with session.execution.succeeded and does not emit
          // session.idle. session.status remains for hosts that still send it.
          if (!item || (item.type !== "session.idle" && item.type !== "session.execution.succeeded" && item.type !== "session.status")) continue
          const info = item.data || item.properties || {}
          if (item.type === "session.status" && info.status && info.status !== "idle") continue
          const sessionID = info.sessionID || info.sessionId || info.id
          const located = item.location && item.location.directory
          const directory = located || info.directory || await directoryFor(ctx, sessionID)
          if (!directory || ignored(directory)) continue
          await runHook({ hook_event_name: "Stop", session_id: sessionID, cwd: directory })
        }
      })().catch(() => {})
    }
  },
}
'''


def render_plugin(config_path):
    script = Path(config_path).parent / "hook" / "cairn_hook.py"
    return (PLUGIN
        .replace("__CAIRN_HOOK_SCRIPT__", json.dumps(str(script)))
        .replace("__CAIRN_SETUP_CONFIG__", json.dumps(str(config_path))))


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
    # --config is the setup-time fallback. --cairn-managed lets a later CAIRN_CONFIG replace it.
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
                group['matcher'] = 'Bash|bash|Edit|edit|Write|write|MultiEdit|apply_patch'
            kept.append(group)
        if kept:
            hooks[event] = kept
        else:
            hooks.pop(event, None)


class JsonNode:
    def __init__(self, value, start, end, members=None):
        self.value, self.start, self.end = value, start, end
        self.members = members or {}
        self.items = []
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
        value, members, items = ({} if token == '{' else []), {}, []
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
                items.append(child)
            if index < len(tokens) and tokens[index][0] == ',':
                child.comma = tokens[index][1:]
                previous = child.comma
                index += 1
            elif index >= len(tokens) or tokens[index][0] != closing:
                raise ValueError('missing JSONC separator')
        if index >= len(tokens):
            raise ValueError('unclosed JSONC container')
        node = JsonNode(value, start, tokens[index][2], members)
        node.items = items
        return node, index+1
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


def plugin_owned(entry):
    # Only the retired single-file config entry. Another package that happens
    # to use the same option name stays.
    if not isinstance(entry, dict):
        return False
    package = entry.get('package')
    options = entry.get('options')
    if not isinstance(package, str) or not isinstance(options, dict):
        return False
    if options.get('cairn') != 'continuity':
        return False
    return package.replace('\\', '/').rstrip('/').endswith('/opencode/plugin/cairn.js')


def edit_plugins(text, root, entry, remove):
    node = root.members.get('plugins')
    if node is not None and not isinstance(node.value, list):
        raise ValueError('plugins must be an array')
    owned = [child for child in (node.items if node else []) if plugin_owned(child.value)]
    desired = json.dumps(entry, separators=(',', ':'))
    if not remove and len(owned) == 1 and json.dumps(owned[0].value, separators=(',', ':')) == desired:
        return text
    if owned:
        cuts = []
        for child in owned:
            cuts.append((child.start, child.end))
            if child.comma:
                cuts.append(child.comma)
            elif child.previous_comma:
                cuts.append(child.previous_comma)
        for start, end in sorted(cuts, reverse=True):
            text = text[:start] + text[end:]
        if remove:
            return text
        root = jsonc(text)
        node = root.members.get('plugins')
    if remove:
        return text
    if node is None:
        blob = '"plugins":[' + desired + ']'
        close = root.end - 1
        if root.members:
            last = max(root.members.values(), key=lambda item: item.end)
            if last.comma:
                return text[:close] + blob + text[close:]
            return text[:last.end] + ',' + text[last.end:close] + blob + text[close:]
        return text[:close] + blob + text[close:]
    close = node.end - 1
    if not node.items:
        return text[:close] + desired + text[close:]
    last = node.items[-1]
    if last.comma:
        return text[:close] + desired + text[close:]
    return text[:close] + ',' + desired + text[close:]


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
    pending_plugin = lambda: None
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
        agents = xdg/'opencode/AGENTS.md'
        files[agents] = block(read(agents), BEGIN, END, spec['instructions'], remove)
        def write_plugin(opencode_path=path, cairn_config=config, remove=remove):
            current = files.get(opencode_path, read(opencode_path))
            edited = edit_plugins(current or '{}', jsonc(current or '{}'), None, True)
            if edited != read(opencode_path):
                files[opencode_path] = edited
            legacy = opencode_path.parent/'plugin'/'cairn.js'
            if legacy.is_file() and read(legacy).startswith(PLUGIN_MARKER):
                legacy.unlink()
            index = opencode_path.parent/'plugins'/'cairn'/'index.js'
            if remove:
                if index.is_file() and read(index).startswith(PLUGIN_MARKER):
                    index.unlink()
                try:
                    index.parent.rmdir()
                except OSError:
                    pass
            elif not index.exists() or read(index).startswith(PLUGIN_MARKER):
                files[index] = render_plugin(cairn_config)
        pending_plugin = write_plugin
    else:
        raise ValueError('agent must be claude, codex or opencode')
    native_action()
    pending_plugin()
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
