---
id: CM-DOC-USERS-AGENTS-INDEX
title: "Choose an agent"
status: accepted
description: "Set up and maintain a Cairn integration on a configured device."
---

# Choose an agent

Setup commands, integration architectures, and instruction file locations for supported coding agents.

## Supported integrations

| Agent | Setup command | Architecture |
| --- | --- | --- |
| [Claude Code](claude-code.md) | `cairn setup <url> --agent claude` | Pre/Post tool hooks, native memory guard, and MCP |
| [Codex](codex.md) | `cairn setup <url> --agent codex` | Exec/tool hooks, instruction injection, and MCP |
| [OpenCode](opencode.md) | `cairn setup <url> --agent opencode` | V2 server plugin hooks and MCP |
| [Grok Build](grok.md) | `cairn setup <url> --agent grok` | Pager hooks and MCP tools |
| [Antigravity CLI](antigravity.md) | `cairn setup <url> --agent antigravity` | Session/invocation hooks and MCP tools |
| [Hermes Agent](hermes.md) | `cairn setup <url> --agent hermes` | Native memory provider and standalone plugin hooks |
| [Hermes chat gateway](hermes-gateway.md) | `cairn setup <url> --agent hermes --gateway` | Gateway provider, chat-tagged memory, and chat tools |

## Setup

Connect an agent using your server URL (paste device token on stdin):

```sh
cairn setup https://cairn.example.com --agent <name>
cairn doctor
```

On a device already configured with a token, run `cairn setup <name>`.

### Instruction files

Setup adds a marked instruction block telling agents to use Cairn as their sole memory service:

| Agent | Managed instruction file |
| --- | --- |
| **Claude Code** | `${CLAUDE_CONFIG_DIR:-~/.claude}/CLAUDE.md` |
| **Codex** | `${CODEX_HOME:-~/.codex}/AGENTS.md` |
| **OpenCode** | `${XDG_CONFIG_HOME:-~/.config}/opencode/AGENTS.md` |
| **Grok Build** | `${GROK_HOME:-~/.grok}/AGENTS.md` |
| **Hermes Agent** | `$HERMES_HOME/SOUL.md` (typically `~/.hermes/SOUL.md`) |
| **Antigravity CLI** | `~/.gemini/config/AGENTS.md` |

### Removal and maintenance

- **Uninstall integration**: Run `cairn setup <agent> --remove`. Leaves device tokens intact and deletes empty parent folders inside `$HOME`.
- **Token reuse**: Configure all agents on a machine using the same device token.
- **Verification**: Restart the agent host and run `cairn doctor` ([Doctor reference](../help/doctor.md)).

See [Configuration reference](../reference/cli.md#configuration).
