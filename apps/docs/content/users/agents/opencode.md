---
id: CM-DOC-USERS-AGENTS-OPENCODE
title: "OpenCode"
status: accepted
description: "Install the OpenCode plugin and MCP integration."
---

# OpenCode

Put the OpenCode CLI on PATH. Install Cairn and check discovery.

```sh
cairn setup https://your-server.example --agent opencode
cairn doctor
opencode mcp list
```

Supply the device token on stdin. On a configured machine use `cairn setup opencode`. Restart OpenCode to load the plugin. It can capture and checkpoint work at Stop. Continuing the model turn with a Stop reminder is unavailable. [OpenCode V2 hooks](../../technical/hooks/opencode.md).
