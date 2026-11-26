---
id: CM-DOC-USERS-AGENTS-GROK
title: "Grok Build"
status: accepted
description: "Install Grok Build hooks and MCP and verify discovery."
---

# Grok Build

```sh
cairn setup https://your-server.example --agent grok
cairn doctor
grok inspect
```

Supply the device token on stdin. On a configured machine use `cairn setup grok`. Setup registers the Cairn MCP server and grants `MCPTool(cairn__*)` in Grok's `config.toml`. Cairn's hooks live in `hooks/cairn.json` under the Grok config directory. Restart Grok. The first tool result carries the opening brief. [Grok Build hook details](../../technical/hooks/grok.md).
