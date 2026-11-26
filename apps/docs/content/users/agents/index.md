---
id: CM-DOC-USERS-AGENTS-INDEX
title: "Choose an agent"
status: accepted
description: "Set up and maintain a Cairn integration on a configured device."
---

# Choose an agent

## Supported integrations

| Agent | Setup name | Integration |
| --- | --- | --- |
| [Claude Code](claude-code.md) | `claude` | Hooks and MCP |
| [Codex](codex.md) | `codex` | Hooks and MCP |
| [OpenCode](opencode.md) | `opencode` | Plugin hooks and MCP |
| [Grok Build](grok.md) | `grok` | Hooks and MCP |
| [Antigravity CLI](antigravity.md) | `antigravity` | Hooks and MCP |
| [Hermes Agent](hermes.md) | `hermes` | Memory provider and plugin hooks |
| [Hermes chat gateway](hermes-gateway.md) | `hermes --gateway` | Profile-specific provider and chat tools |

## Setup

Run `cairn setup <url> --agent claude|codex|opencode|grok|antigravity|hermes` and supply the device token on stdin. On an already configured device, run `cairn setup <agent>`. Setup writes managed hook and MCP entries (for Hermes, a memory provider instead of MCP) and adds a marked instruction block. Upgrades refresh installed integrations; for Hermes, only the profile `HERMES_HOME` selects, or the default profile. Run `cairn setup <agent> --remove` to remove the integration while keeping the device configuration. Removal deletes an instruction, plugin or host config file it changes to empty: whitespace only, or an empty JSON object or YAML mapping. It also removes empty parent folders whose resolved paths stay inside the home folder. Hermes removal cleans up Cairn's bytecode from the real plugin cache inside the profile when it removes Cairn's plugin files. A file with any other key or content stays, and so does a file removal does not change. Codex and OpenCode setup need their native CLI on `PATH`. Codex also requires trusting the hooks. `cairn doctor` checks the configuration.

| Agent | Global instruction file |
| --- | --- |
| Claude Code | `${CLAUDE_CONFIG_DIR:-~/.claude}/CLAUDE.md` |
| Codex | `${CODEX_HOME:-~/.codex}/AGENTS.md` |
| OpenCode | `${XDG_CONFIG_HOME:-~/.config}/opencode/AGENTS.md` |
| Grok Build | `${GROK_HOME:-~/.grok}/AGENTS.md` |
| Hermes Agent | `$HERMES_HOME/SOUL.md` (typically `~/.hermes/SOUL.md`) |
| Antigravity CLI | `~/.gemini/config/AGENTS.md` |

Set up each agent on the machine with the same device config. Give every machine its own token. After installing or refreshing an integration, restart the host and run [cairn doctor](../help/doctor.md). [Config file selection and URL rules](../reference/cli.md#configuration).
