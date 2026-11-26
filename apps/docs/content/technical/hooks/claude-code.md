---
id: CM-GUIDE-002
title: "Capture Claude Code activity into Cairn"
status: accepted
description: "Use the Claude Code hook response envelope or wire Cairn for one project."
---

# Capture Claude Code activity into Cairn

Configure Claude Code hook events, response envelopes, and per-project hook settings.

Download the sample [claude-code-hooks.json](../../../public/examples/claude-code-hooks.json) to configure project-level hooks.

## Hook event mapping

| Claude event | Cairn hook action | Injected context |
| --- | --- | --- |
| `SessionStart` | Initial session brief | L0 pins + L1 essentials (up to 1,800 B) |
| `UserPromptSubmit` | Prompt capture and lookup | Matching preferences, project rules, and skill cards |
| `PreToolUse` | Native memory guard & command warnings | Denies writes to `projects/*/memory/`; warns on commands |
| `PostToolUse` | File edit and command capture | File topic notes on first edit of a file path |
| `PostToolUseFailure` | Terminal error interception | Related gotchas and remedies for failed commands |
| `Stop`, `PreCompact` | Checkpoint session state | Prompts Stop reminder to save reusable notes |
| `SessionEnd` | Finalize session | Enqueues background distillation |

## Manual setup

1. Generate a device token in the console under **Settings → Devices**.
2. Run `cairn setup <server-url>` with the token on stdin.
3. In your project checkout, initialize membership with `cairn init`.
4. Copy the `hooks` section of `claude-code-hooks.json` into `.claude/settings.local.json`.
5. Test the hook manually:
   ```bash
   printf '%s' '{"session_id":"test","cwd":"'"$PWD"'","prompt":"test"}' \
     | cairn hook UserPromptSubmit --agent claude
   ```
6. Restart Claude Code and verify in `/hooks`.

See [Agent hooks contract](index.md#events) and [Claude Code integration](../../users/agents/claude-code.md).
