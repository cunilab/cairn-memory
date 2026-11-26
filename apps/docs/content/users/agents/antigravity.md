---
id: CM-DOC-USERS-AGENTS-ANTIGRAVITY
title: "Antigravity CLI"
status: accepted
description: "Connect Antigravity CLI to Cairn and verify its managed integration."
---

# Antigravity CLI

Connect Google's Antigravity CLI to Cairn using managed hooks and MCP tools.

## Setup and verification

```sh
cairn setup https://cairn.example.com --agent antigravity
cairn doctor
agy mcp list
```

Supply the device token on stdin. On a previously configured device, run `cairn setup antigravity`.

## Integration details

- **Hooks registered**: Registers `SessionStart`, `PreInvocation`, `PreToolUse`, `PostToolUse`, and `Stop` hooks.
- **MCP permissions**: Automatically grants `mcp(cairn/*)` in `~/.gemini/antigravity-cli/settings.json`. Explicit host ask/deny rules take precedence.
- **Restart**: Restart the CLI after setup to load updated MCP schemas.

See [Antigravity CLI hook details](../../technical/hooks/antigravity.md).
