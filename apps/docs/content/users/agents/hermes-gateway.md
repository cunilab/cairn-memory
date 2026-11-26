---
id: CM-DOC-USERS-AGENTS-HERMES-GATEWAY
title: "Hermes chat gateway"
status: accepted
description: "Connect a dedicated Hermes gateway profile and control which chats and projects share memory."
---

# Hermes chat gateway

## Hermes gateway

Use a dedicated Hermes profile with `plugins.isolation: in_process`. Stop the gateway, then run
`HERMES_HOME=/path/to/profile cairn setup hermes --gateway` with Cairn already configured.
Setup makes Cairn the profile's memory provider as above. Its provider offers native `memory_search`,
`memory_save`, and the four [global skill tools](../reference/mcp.md#tools) instead of the coding tools. Restart the gateway and use `/new` so Hermes loads the current tool schemas. Run `HERMES_HOME=/path/to/profile cairn setup hermes --remove` to remove the integration.
Keep the profile's chat salt so Cairn can find its existing chat memories.
[Hermes tool selection](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/hermes_cli/tools_config.py#L737-L757) · [Tool schema restoration](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/agent/conversation_loop.py#L769-L788)

Use Cairn CLI 0.1.11 or newer for provider-based setup and Hermes v2026.4.23 or newer. The gateway requires the server to report at least 0.1.10.

## Hermes chat recall

Cairn saves a memory without a project as a personal record tagged to its chat. The
same chat and the gateway account's web session can read it. DMs and groups use the
same rule for L0 pins, L1 essentials, L2 prompts and commands, and explicit search.
Other chats and standalone agent sessions leave out those personal memories.

Explicit search can also include a project selected by ID or exact name when the
gateway account is a member. Saves with an explicit project become project
memories without a chat tag. Preferences stay personal. Memories from group
chats start as candidates, including project saves and memories derived from turns.

Each chat thread has its own Cairn session and eight-card automatic recall ledger.
A chat without threads uses one session. At 30 minutes of idle time Cairn closes
the session and queues distillation. The next message starts a new ledger. A gateway
shutdown, `/new` or `/reset` also closes the chat session after any active turn.
The next message starts a new session with the chat instruction. If the gateway
stops without finalizing a session, the idle sweep closes it. See [Hermes gateway setup](hermes-gateway.md#hermes-gateway).

[Native chat tool arguments](../reference/mcp.md#hermes-gateway-tools) · [Gateway protocol](../../technical/hooks/hermes.md#gateway-routing)
