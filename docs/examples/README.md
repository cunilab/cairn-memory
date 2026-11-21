---
id: CM-GUIDE-002
title: "Capture Claude Code activity into Cairn"
status: accepted
---

# Capture Claude Code activity into Cairn

`claude-code-hooks.json` wires the six Claude Code hook events to `cairn hook`, the hook that is
part of the `cairn` binary. Events, fields and limits are in the
[hook contract](../contracts/hook-v1.md#events).

| Event | What `cairn hook` does |
| --- | --- |
| `SessionStart` | injects the bounded server brief (at most 1,200 B) |
| `UserPromptSubmit` | captures the prompt |
| `PostToolUse` (`Bash`, `Edit`, `Write`, `MultiEdit`, `apply_patch`) | captures edits and non-read-only commands; the first edit of a file may inject a short note about it |
| `Stop`, `PreCompact` | save a checkpoint after new work; assistant capture stays opt-in |
| `SessionEnd` | best-effort rule-based session distillation |

Automatic installation is `cairn setup <url> --agent claude` with the device token on stdin; it
installs these hooks plus the MCP entry, preserves unrelated config, and replaces any older
Cairn-managed entry. The manual example below is for a custom project-only integration. The
[hook contract](../contracts/hook-v1.md#setup) owns the setup rules.

## Setup

1. Start the Cairn server and sign in to the web console.
2. On the Devices page, create a device for this machine and copy its token. It is shown once. Use a
   device token, not an account or web session token. Install the `cairn` CLI from a
   [release](../release.md), put it on `PATH`, and run `cairn setup <server-url>` with the token on
   stdin. That writes the config file (`~/.config/cairn/config.json`, or under `$XDG_CONFIG_HOME` /
   `%APPDATA%`).
3. Optional, for teams: in each repository checkout run `cairn init` once (the project owner) and
   commit the `.cairn/project.json` it writes. The hook reads that file from the checkout's git top
   level and sends its `project_id`, so a teammate's first capture joins the project. There is no
   invite. It holds no token or server URL.
4. Copy the `hooks` block of `claude-code-hooks.json` into `~/.claude/settings.json`, or into
   `.claude/settings.local.json` for one project. Merge with any existing `hooks` key instead of
   replacing it. If `cairn` is not on `PATH`, put its absolute path in the commands. To use another
   config file add `--config <path>` to each command.
5. Check the hook alone before relying on it:

   ```bash
   printf '%s' '{"session_id":"hook-check","cwd":"'"$PWD"'","prompt":"hook check"}' \
     | cairn hook UserPromptSubmit --agent claude
   ```

   It prints nothing and exits 0; the prompt appears in the console's captures after the next
   session distills. A problem is one line on stderr.
6. Restart Claude Code (or open `/hooks` to review the config), send a prompt, and confirm the work
   shows up in the web console or in search.
7. If the server was unreachable, captures wait in `outbox.json` next to the config file and are
   sent by the next hook run that succeeds (at most 256 events). Set `CAIRN_OUTBOX` to an absolute
   path to move that file; do not commit it.

## Notes

- The hook never writes to stdout except the injected context, so leave stdout connected to Claude.
- `PostToolUse` counts file edits and commands as work. Reads and searches do not.
- `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are ignored; the config file supplies both.
