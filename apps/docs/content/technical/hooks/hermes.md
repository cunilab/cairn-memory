---
id: CM-DOC-TECHNICAL-HOOKS-HERMES
title: "Hermes Agent and gateway hooks"
status: accepted
description: "Provider callbacks, terminal tool capture, and gateway session routing."
---

# Hermes Agent and gateway hooks

Provider callbacks, terminal tool capture, and gateway session routing.

In Hermes Agent, Cairn operates both as the external memory provider and as a standalone plugin (`kind: standalone` in `plugin.yaml`).

## Provider callbacks

| Provider callback | Cairn implementation |
| --- | --- |
| `initialize` | Registers working directory and agent context. Bypasses subagent, cron, and flush agents. |
| `on_turn_start` | Evaluates `SessionStart` (once per session) and `UserPromptSubmit`. |
| `prefetch` | Injects session brief and prompt recall cards into user message context. |
| `sync_turn` | Executes `Stop` checkpoint after completed turns. Hermes cannot continue a finished turn, so a coding session's Stop reminder is held and delivered once, phrased as "Last turn you changed N file(s)...", with the next prompt's recall; gateway chats get none. |
| `on_pre_compress` | Runs `PreCompact` checkpoint before context compression. |
| `on_session_end` | Finalizes session and queues distillation at session boundaries. |
| `on_session_switch` | Tracks session switches (`/new`, `/reset`, `/resume`, `/branch`, and compression). |
| `get_tool_schemas`, `handle_tool_call` | Provides native memory and skill tools (`memory_search`, `memory_save`, etc.). |

Plugin hooks manage tool capture:
- `post_tool_call` captures file edits and terminal commands.
- `transform_tool_result` attaches command gotchas and error recall directly to terminal outputs.

See [Native tool schemas](../../users/reference/mcp.md#hermes-provider-tools).

## Gateway routing

For chat platform gateways, Cairn isolates personal memory by chat thread:

- **Salted hashes**: Sends salted hashes of platform chat and thread routes rather than raw user IDs.
- **Version requirements**: Verifies that the server reports ≥ 0.1.10 before activating chat memory.
- **Session lifecycles**: Chat sessions persist across Hermes conversation turns, finalizing after 30 minutes of idle time or upon `/new`, `/reset`, or gateway shutdown (`on_session_finalize`).
- **Chat instruction**: First prompt injects the Hermes chat instruction (up to 600 bytes).
- **Project sharing**: Searches and saves with an explicit project ID or exact project name share memory with project members rather than tagging to the chat.

See [Hermes chat recall](../../users/agents/hermes-gateway.md#hermes-chat-recall).
