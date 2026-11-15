# Capture Claude Code activity into Cairn

`claude-code-hooks.json` wires four Claude Code hook events to
`cairn_hook.py`. Field mapping and limits are in the
[hook contract](../docs/contracts/hook-v1.md#claude-code-hook-input).

| Event | Captured as |
| --- | --- |
| `UserPromptSubmit` | role `user`, the prompt |
| `PostToolUse` (`Bash`, `Edit`, `Write`, `MultiEdit`) | role `tool`, tool name, input, and output (cut to 16000 characters) |
| `Stop` | role `assistant`, the final reply of the turn |
| `SessionEnd` | role `tool`, the session end input |

## Setup

1. Start the Cairn server and sign in to the web console.
2. On the Devices page, create a device for this machine and copy its token.
   It is shown once. Use a device token, not an account or web session token.
   Install the `cairn` CLI from a [release](../docs/release.md) and run
   `cairn setup <server-url>` with the token on stdin. Besides the config it
   installs `cairn_hook.py` and `cairn_outbox.py` into the `hook/` directory next to
   `config.json` (`~/.config/cairn/hook/`, or under `$XDG_CONFIG_HOME` /
   `%APPDATA%`), checked against the release's `SHA256SUMS`. `cairn update`
   refreshes them. Setup only warns if it is offline; run `cairn update` later.
3. Pick a queue path outside any repository, for example
   `~/.cairn/outbox.json`. The hook creates it on the first retryable failure.
4. Copy the `hooks` block (and the `env` block, see below) of
   `claude-code-hooks.json` into `~/.claude/settings.json`, or into
   `.claude/settings.local.json` for one project. Merge with any existing
   `hooks` key instead of replacing it.
5. If your config directory is not `~/.config/cairn`, change the script path in the
   commands. Set `CAIRN_URL` (https, or http only for loopback), `CAIRN_DEVICE_TOKEN`, and
   `CAIRN_OUTBOX` as an absolute path. Do not commit a file that holds the
   token. As an alternative to the `env` block, export the three variables in
   the shell that starts `claude` and drop the `env` block.
6. Check the hook alone before relying on it:

   ```bash
   printf '%s' '{"session_id":"hook-check","hook_event_name":"UserPromptSubmit","prompt":"hook check"}' \
     | CAIRN_URL=http://127.0.0.1:3111 CAIRN_DEVICE_TOKEN='<device-token>' \
       python3 ~/.config/cairn/hook/cairn_hook.py
   ```

   It prints the server reply and exits 0.
7. Restart Claude Code (or open `/hooks` to review the config), send a prompt,
   and confirm the capture appears in the web console or in search.
8. If the server was unreachable, `python3 ~/.config/cairn/hook/cairn_outbox.py status
   "$CAIRN_OUTBOX"` shows queued events (`pending`) and conflicts that were
   dropped (`rejectedCount`, `lastRejected`). Queued events are sent by the
   next hook run that succeeds, or by `cairn_outbox.py replay`.

## Notes

- The command ends in `>/dev/null` so the server reply is not added to the
  model's context on `UserPromptSubmit`. Errors still reach stderr, and a
  failing hook never blocks Claude Code (only exit code 2 blocks).
- `PostToolUse` is limited to tools that change state or run commands. Reads
  and searches are noisy and large; add them to the matcher if you want them.
- `SessionStart` can be added the same way; it records only the start input.
