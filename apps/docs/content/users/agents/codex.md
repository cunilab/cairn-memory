---
id: CM-DOC-USERS-AGENTS-CODEX
title: "Codex"
status: accepted
description: "Register Cairn with the native Codex CLI and trust its installed hooks."
---

# Codex

Put the Codex CLI on PATH before running setup. Supply your Cairn device token on stdin.

```sh
cairn setup https://your-server.example --agent codex
cairn doctor
codex mcp list
```

On a configured machine use `cairn setup codex`. Restart Codex and trust the installed Cairn hooks when reviewing them in Codex. Setup adds `mcp_servers.cairn` with `default_tools_approval_mode = "approve"`, including for `codex exec`.

The global Cairn block is in `AGENTS.md` under `CODEX_HOME` or `~/.codex`. A nonempty global `AGENTS.override.md` takes precedence. Review that file if the expected global instructions do not appear. [Codex hook details](../../technical/hooks/codex.md).
