---
id: CM-DOC-TECHNICAL-HOOKS-HERMES
title: "Hermes Agent and gateway hooks"
status: accepted
description: "Provider callbacks, terminal tool capture, and gateway session routing."
---

# Hermes Agent and gateway hooks

## Provider callbacks

The plugin manifest declares `kind: standalone`. Without it, Hermes treats a folder that defines a memory provider as provider-only and skips its hooks. With it, the folder loads both as a plugin (tool hooks) and as the memory provider. [Kind detection](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/hermes_cli/plugins_manifest.py#L529-L551), [dual-kind hooks](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/hermes_cli/plugins_ledger.py#L211-L232)

| Provider method | Cairn behaviour |
| --- | --- |
| `initialize` | Records the session, working folder and `agent_context`. Only `primary` agents write; subagent, cron and flush agents, and the background-agent checks, turn Cairn off. Raw chat and user IDs are ignored. |
| `on_turn_start` | Runs every turn: SessionStart once per session, then UserPromptSubmit. |
| `prefetch` | Delivers the brief and prompt recall with the user message. Hermes skips prefetch for trivial prompts, so a brief waits for the next one. |
| `sync_turn` | Stop checkpoint for a completed turn. |
| `on_pre_compress` | PreCompact checkpoint before compaction. |
| `on_session_end` | SessionEnd at Hermes's session boundaries. |
| `on_session_switch` | Follows `/new`, `/reset`, `/resume`, `/branch` and compression to the new session ID. A new conversation gets a brief on its next prompt; compression continues without one. |
| `get_tool_schemas`, `handle_tool_call` | Cairn's memory and skill tools. |

Tool events stay plugin hooks because providers have none. `post_tool_call` captures terminal and file tools and Cairn's own tool calls, looks up command warnings and recalls related memories for failed commands. `transform_tool_result` attaches that recall to the terminal result. Hermes's turn-end hook `on_session_end` checkpoints failed or interrupted turns, which Hermes does not pass to `sync_turn`. Capture requires a working folder supplied by Hermes. Per-file notes and continuing the model loop with a Stop reminder are unavailable on Hermes. [Prompt gate](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/agent/turn_context.py#L1076-L1090), [interrupted turns](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/run_agent.py#L928-L945), [Hermes hooks](https://hermes-agent.nousresearch.com/docs/user-guide/features/hooks)

[Native tool schemas](../../users/reference/mcp.md#hermes-provider-tools).

## Gateway routing

Cairn accepts a bound route from any platform with a lowercase token of up to 32 ASCII
characters. It sends salted hashes of the chat and thread route. A missing route disables
Cairn in that turn. DMs and groups share the same visibility rules. Each chat hook call first
checks that the server reports version 0.1.10 or later; otherwise, or when the server does not
answer, chat memory is off for that call and nothing is sent.

The chat route still comes from the turn's bound gateway context, not from the provider's chat arguments, so Discord threads keep their parent channel's tag. Chat sessions live per gateway profile, across Hermes sessions. Hermes `/new`, `/reset` and gateway shutdown trigger `on_session_finalize`, which runs Cairn Stop and SessionEnd after any active turn finishes. An idle chat closes after 30 minutes without a turn. Evicting a cached agent leaves the chat session open.

On the first prompt of a Cairn chat session, Hermes receives the **Hermes chat instruction**
from **Settings → Agent instructions**. A server admin can edit it or reset it to the built-in
text. It has the same 600-byte limit as the session start line. Hermes handles conversation
and owner confirmation. Cairn checks scope and membership for each explicit memory request.
Pass an exact project name or ID to extend a search to that project or to share a save with
its members. Skill tools use the global library without chat or project metadata. Prompt recall in chats skips skill cards. [Chat recall](../../users/agents/hermes-gateway.md#hermes-chat-recall)

[Gateway setup and visibility](../../users/agents/hermes-gateway.md#hermes-chat-recall).
