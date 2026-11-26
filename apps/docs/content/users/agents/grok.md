---
id: CM-DOC-USERS-AGENTS-GROK
title: "Grok Build"
status: accepted
description: "Install Grok Build hooks and MCP and verify discovery."
---

# Grok Build

Install Grok Build hooks and MCP server configuration for terminal and prompt recall.

## Setup and verification

```sh
cairn setup https://cairn.example.com --agent grok
cairn doctor
grok inspect
```

Supply the device token on stdin. On a previously configured device, run `cairn setup grok`.

## Configuration details

- **MCP tools**: Registers the Cairn MCP server and adds `MCPTool(cairn__*)` permissions in Grok's `config.toml`.
- **Hooks**: Installs hook definitions to `hooks/cairn.json` inside the Grok configuration directory.
- **Delivery**: The first tool result of a session carries the opening session brief. Restart Grok to activate.

See [Grok Build hook details](../../technical/hooks/grok.md).
