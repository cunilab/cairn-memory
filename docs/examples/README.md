---
id: CM-GUIDE-002
title: "Capture Claude Code activity into Cairn"
status: accepted
---

# Capture Claude Code activity into Cairn

`claude-code-hooks.json` wires five Claude Code hook events to
`cairn_hook.py`. Field mapping and limits are in the
[hook contract](../contracts/hook-v1.md#claude-code-hook-input).

| Event | Captured as |
| --- | --- |
| `UserPromptSubmit` | role `user`, the prompt |
| `PostToolUse` (`Bash`, `Edit`, `Write`, `MultiEdit`) | role `tool`, tool name, input, and output (cut to 16000 characters) |
| `SessionStart` | short Cairn note plus bounded server brief |
| `Stop` | asks to save only after new work, with a recursion guard; assistant capture stays opt-in |
| `SessionEnd` | best-effort rule-based session distillation |

Automatic installation is `cairn setup <url> --agent claude` with the device
token on stdin; it installs hooks plus MCP and preserves unrelated config.
The manual example below is for a custom project-only integration. The
[hook contract](../contracts/hook-v1.md#verified-agent-capabilities) owns
capabilities, official references, work tracking and bounds.

## Setup

1. Start the Cairn server and sign in to the web console.
2. On the Devices page, create a device for this machine and copy its token.
   It is shown once. Use a device token, not an account or web session token.
   Install the `cairn` CLI from a [release](../release.md) and run
   `cairn setup <server-url>` with the token on stdin. Besides the config it
   installs `cairn_hook.py`, `cairn_outbox.py` and `cairn_instructions.txt` into the `hook/` directory next to
   `config.json` (`~/.config/cairn/hook/`, or under `$XDG_CONFIG_HOME` /
   `%APPDATA%`), checked against the release's `SHA256SUMS`. `cairn update`
   refreshes them. Setup only warns if it is offline; run `cairn update` later.
3. Optional, for teams: in each repository checkout run `cairn init` once (the project
   owner) and commit the `.cairn/project.json` it writes. The hook reads that file from
   the checkout's git top level and sends its `project_id`, so a teammate's first capture
   joins the project. There is no invite. It holds no token or server URL.
4. Pick a queue path outside any repository, for example
   `~/.cairn/outbox.json`. The hook creates it on the first retryable failure.
5. Copy the `hooks` block (and the `env` block, see below) of
   `claude-code-hooks.json` into `~/.claude/settings.json`, or into
   `.claude/settings.local.json` for one project. Merge with any existing
   `hooks` key instead of replacing it.
6. If your config directory is not `~/.config/cairn`, change the script path in the
   commands. Run `cairn setup <server-url>` so the default config file holds the
   server URL (https, or http only for loopback; a reverse proxy path prefix such as
   `https://cairn.example.test/agent` is allowed) and the device credential.
   Set `CAIRN_OUTBOX` as an absolute path. Do not commit a file that holds the
   token. `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are ignored.
7. Check the hook alone before relying on it:

   ```bash
   printf '%s' '{"session_id":"hook-check","hook_event_name":"UserPromptSubmit","prompt":"hook check"}' \
     | python3 ~/.config/cairn/hook/cairn_hook.py
   ```

   It prints the server reply and exits 0.
8. Restart Claude Code (or open `/hooks` to review the config), send a prompt,
   and confirm the capture appears in the web console or in search.
9. If the server was unreachable, `python3 ~/.config/cairn/hook/cairn_outbox.py status
   "$CAIRN_OUTBOX"` shows queued events (`pending`) and conflicts that were
   dropped (`rejectedCount`, `lastRejected`). Queued events are sent by the
   next hook run that succeeds, or by `cairn_outbox.py replay`.

## Notes

- Agent-mode commands keep capture acknowledgements off stdout; instruction
  delivery requires the hook JSON to reach Claude, so leave stdout connected.
- `PostToolUse` counts file edits and commands as work. Reads/searches do not.
- Generic invocation without `--agent` remains the one-shot capture interface.
