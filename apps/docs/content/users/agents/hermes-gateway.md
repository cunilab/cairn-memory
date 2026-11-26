---
id: CM-DOC-USERS-AGENTS-HERMES-GATEWAY
title: "Hermes chat gateway"
status: accepted
description: "Connect a dedicated Hermes gateway profile and control which chats and projects share memory."
---

# Hermes chat gateway

A [gateway](../../glossary.md#gateway) gives Hermes a dedicated messaging profile with [chat-scoped memory](../../glossary.md#chat-tag).

## Hermes gateway

Prerequisites: Dedicated Hermes profile with `plugins.isolation: in_process`, Hermes v2026.4.23+, Cairn CLI 0.1.11+, and Cairn server 0.1.10+.

```sh
HERMES_HOME=/path/to/gateway/profile cairn setup hermes --gateway
```

- **Tools exposed**: Exposes native `memory_search`, `memory_save`, and the four [global skill tools](../reference/mcp.md#tools) (excluding terminal/code-editing tools).
- **Restart**: Restart the gateway and execute `/new` to reload schemas.
- **Chat salt**: Preserves the profile's chat salt to retain access to existing chat memories.
- **Removal**: Run `HERMES_HOME=/path/to/gateway/profile cairn setup hermes --remove`.

## Hermes chat recall

- **Chat boundary**: Memories saved without an explicit project are stored as personal records tagged with the chat thread ID. Only that specific chat and the gateway account's web session can read them.
- **Project sharing**: Explicit searches and saves can target a project by exact name or ID if the gateway account is a member. Project saves are shared across all project members without chat tags.
- **Candidates**: All memories saved from group chats default to candidate status (`new`).
- **Session management**: Each thread maintains an independent session and 8-card recall ledger. Sessions finalize after 30 minutes of idle time or upon `/new` or `/reset`.

See [Native chat tool arguments](../reference/mcp.md#hermes-gateway-tools) and [Gateway protocol](../../technical/hooks/hermes.md#gateway-routing).
