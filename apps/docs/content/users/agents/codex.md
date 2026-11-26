---
id: CM-DOC-USERS-AGENTS-CODEX
title: "Codex"
status: accepted
description: "Register Cairn with the native Codex CLI and trust its installed hooks."
---

# Codex

Register Cairn with OpenAI Codex CLI, approve MCP servers, and configure global instructions.

## Setup and verification

Prerequisite: Ensure the `codex` CLI is on your `PATH`.

```sh
cairn setup https://cairn.example.com --agent codex
cairn doctor
codex mcp list
```

Supply the device token on stdin. On an existing device, run `cairn setup codex`.

## Configuration details

- **MCP server**: Adds `mcp_servers.cairn` with `default_tools_approval_mode = "approve"` (including for `codex exec`).
- **Trust hooks**: Trust the installed Cairn hooks when prompted in Codex.
- **Instructions**: Writes global instructions to `${CODEX_HOME:-~/.codex}/AGENTS.md`. A non-empty `AGENTS.override.md` takes precedence if present.

See [Codex hook details](../../technical/hooks/codex.md).
