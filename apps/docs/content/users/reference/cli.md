---
id: CM-DOC-USERS-REFERENCE-CLI
title: "CLI"
status: accepted
description: "Reference for CLI commands, configuration, exit behavior and device setup."
---

# CLI

## Configuration

- `cairn setup <server-url>` reads the device token from stdin (hidden on supported Unix terminals; setup warns when it cannot hide input) and writes the
  selected config file with mode `0600` through a temporary file and a rename. The selected file is
  `CAIRN_CONFIG` when non-empty, else `$XDG_CONFIG_HOME/cairn/config.json`, else
  `~/.config/cairn/config.json`, else `%APPDATA%`. `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are not
  configuration: a present one is ignored with one stderr line naming it, never its value.
- Server URL: `https`, or `http` for a loopback host only; no credentials, query or fragment; a path
  prefix is allowed. Every request goes to `<url>/cairn-memory/...`. Redirects are never followed.
  Plain `http` never uses `HTTP(S)_PROXY`.

## CLI

```
cairn setup <url> [--agent claude|codex|opencode|grok|antigravity|hermes] [--remove|--keep-native-memory|--gateway]
cairn setup <agent> [--remove|--gateway]                 cairn init [name] [--yes]
cairn import claude-memory [--yes]
cairn fixes [apply [id]]     cairn doctor     cairn mcp     cairn update [--check]     cairn hook <Event> …     cairn --version
cairn search <query> [--kind k] [--limit n]     cairn get <id>... [--detail]     cairn save [<json>|-]
cairn feedback <id> helpful|wrong               cairn forget <id>
```

The five verbs call the same tools and print the same compact JSON plus a newline; `save` reads one
`SaveInput` object from its argument or stdin. `doctor` prints `ok|warn|fail  <check>  <detail>`
per check, exiting 1 on any `fail`. It checks the config, server, semantic search, device, project,
and agent integrations. `hook` runs the capture and injection hook ([hook contract](../../technical/hooks/index.md)).
Account and device management stay in the web console.

For a Cairn-managed integration, `doctor` warns if its global instruction block is missing or
differs from this version. The warning names the file and the `cairn setup <agent>` command that
refreshes the block. Repair malformed block markers before refreshing. Hermes gateway profiles
receive their instructions per chat and skip this check. For Hermes, `doctor` also warns when the installed Hermes is older than v2026.4.23 or its version is unknown ([why](../../technical/hooks/index.md#setup)), and checks that the profile
selects Cairn as its memory provider instead of checking for an MCP entry.

For Claude Code, `doctor` also checks the current project's `MEMORY.md` when the memory folder
exists and setup is managing native memory. Run `cairn import claude-memory` from that project
to restore the Cairn pointer. If import reports that an existing backup differs from a rewritten
index, move the rewritten index out of the memory folder before retrying.
Setup with `--keep-native-memory` skips the pointer check. These diagnostics only read local files
and print warnings.

[Input and response shapes](mcp.md#tools) · [Doctor](../help/doctor.md) · [Upgrade](../upgrade.md) · [Claude Code import](../agents/claude-code.md#import-claude-code-memory)
