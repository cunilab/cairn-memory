---
id: CM-DOC-USERS-AGENTS-OPENCODE
title: "OpenCode"
status: accepted
description: "Install the OpenCode plugin and MCP integration."
---

# OpenCode

Install the OpenCode V2 server plugin and MCP tools for session capture and memory retrieval.

## Setup and verification

Prerequisite: Ensure `opencode` CLI is on your `PATH`.

```sh
cairn setup https://cairn.example.com --agent opencode
cairn doctor
opencode mcp list
```

Supply the device token on stdin. On a previously configured device, run `cairn setup opencode`.

## Integration details

- **Plugin**: Registers Cairn's OpenCode server plugin to capture events and checkpoint work on `Stop`.
- **Turn continuation**: Note that OpenCode does not support continuing the model turn from a Stop reminder.
- **MCP server**: Registers standard Cairn memory and skill tools. Restart OpenCode to activate.

See [OpenCode V2 hooks](../../technical/hooks/opencode.md).
