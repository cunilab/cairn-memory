---
id: CM-DOC-USERS-REFERENCE-CLI
title: "CLI"
status: accepted
description: "Reference for CLI commands, configuration, exit behavior and device setup."
---

# CLI

Command-line interface reference for setup, diagnostics, memory operations, and agent hooks.

## Configuration

Cairn stores client configuration and device tokens locally with restricted permissions:

- **File resolution**: Checked in order:
  1. `CAIRN_CONFIG` (if set and non-empty)
  2. `$XDG_CONFIG_HOME/cairn/config.json`
  3. `~/.config/cairn/config.json`
  4. `%APPDATA%\cairn\config.json` (Windows)
- **Permissions**: Created with mode `0600` via an atomic temporary file rename.
- **Ignored environment variables**: `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are explicitly ignored to avoid credential leaks; a warning is logged to stderr without printing the value.
- **Server URL**: Must use `https://` (or `http://` strictly for loopback addresses). Redirects are not followed; proxies are bypassed on loopback. Every API request targets `<url>/cairn-memory/...`.

## CLI commands

```sh
# Setup & Maintenance
cairn setup <url> [--agent claude|codex|opencode|grok|antigravity|hermes] [--remove|--keep-native-memory|--gateway]
cairn setup <agent> [--remove|--gateway]
cairn init [name] [--yes]
cairn doctor
cairn fixes [apply [id]]
cairn update [--check]
cairn import claude-memory [--yes]

# Memory & Skill Operations
cairn search <query> [--kind k] [--limit n]
cairn get <id>... [--detail]
cairn save [<json>|-]
cairn feedback <id> helpful|wrong
cairn forget <id>

# Low-level Transport & Version
cairn mcp
cairn hook <Event> ...
cairn --version
```

### Diagnostics and execution details

- **`doctor`**: Runs health checks on configuration, network, semantic search, project attachment, and agent integrations. Prints `ok|warn|fail` lines and exits `1` on any failure.
- **`hook`**: Low-level capture and injection runner executed by host agents ([Agent hooks](../../technical/hooks/index.md)).
- **`fixes`**: Lists and applies automated remediations recommended by the server.

[Input and response shapes](mcp.md#tools) · [Doctor](../help/doctor.md) · [Upgrade](../upgrade.md) · [Claude Code import](../agents/claude-code.md#import-claude-code-memory)
