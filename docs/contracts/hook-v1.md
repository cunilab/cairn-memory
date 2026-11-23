---
id: CM-CONTRACT-005
title: "Layer 1 Hook Capture Adapter"
status: draft
---

# Layer 1 Hook Capture Adapter

`cairn hook <Event>` is the one hook for Claude Code, Codex and OpenCode. It reads the host's
event as JSON on stdin, sends typed captures to the server with the device token, and prints
injected context on stdout only when there is some. There is no Python and no script to install:
the hook is the `cairn` binary. Design: [step 4 spec](../specs/0.1.6-step4-capture.md).

## Invocation

```
cairn hook <Event> [--agent <agent>] [--config <path>] [--cairn-managed]
```

The server URL and device token come from one JSON file: `--config`, else a non-empty
`CAIRN_CONFIG`, else `$XDG_CONFIG_HOME/cairn/config.json`, else `~/.config/cairn/config.json`
(`%APPDATA%` on Windows). With `--cairn-managed` (what `cairn setup` writes) a non-empty
`CAIRN_CONFIG` wins and `--config` is the fallback. `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are not
configuration: if present the hook says so on stderr (never the value) and ignores them. The
positional `<Event>` is the only event source. Unknown events are ignored.

A missing config file, or a `.cairnignore` in the folder or any ancestor up to the git top level,
turns the hook off: exit 0, no output, no state, no network. Every other failure writes one stderr
line and exits 0, so a hook never breaks its host. Only a bad option or a missing event exits 1.
HTTP 401 names the config path, never the token.

## Payload

Snake_case keys on stdin: `session_id` (required), `cwd`, `source`, `prompt`, `tool_name`,
`tool_input` (`file_path`/`filePath`/`path`, `command`/`cmd`, an `apply_patch` text), `tool_response`,
`last_assistant_message`. Codex sends Claude's names; the OpenCode plugin maps its own events to
them. Context (repository, branch, worktree, project id from `.cairn/project.json`, git state) is
derived from `cwd`, never read from the payload.

## Events

| Event | Capture | Server call | Injects |
| --- | --- | --- | --- |
| `SessionStart` (startup, resume, clear, compact) | none | `POST /session/brief`; when the reply has `check`, then `POST /session/repo-state` | the brief, at most 1,200 B; in a repository with `.cairn/project.json` one more instruction line (below), at most 200 B |
| `UserPromptSubmit` | `prompt`, at most 8 KiB | `POST /observe` | nothing |
| `PostToolUse`, file tool (`Edit`, `Write`, `MultiEdit`, `apply_patch`) | `edit`: `<Tool> <path>` plus a 1 KiB excerpt | `/observe`; the first edit of a path this session also `POST /session/topic` | a topic note or card, at most 300 B |
| `PostToolUse`, shell, not read-only | `command` with `ok`; a failed one keeps the command, the exit and the first error line (200 B) | `/observe` | nothing |
| `PostToolUse`, the `memory_save` tool of Cairn (`mcp__cairn__memory_save`, `cairn_memory_save`) or `cairn save` in a shell | none (only marks the session as having saved) | none | nothing |
| `PostToolUse`, read-only or other tool | dropped | none | nothing |
| `Stop` | `assistant` only when `CAIRN_CAPTURE_ASSISTANT=1` | `POST /session/checkpoint` if work is unsaved | once per session, a reminder to save (below); otherwise nothing |
| `PreCompact` (Claude) | none | `POST /session/checkpoint` if the session worked | nothing |
| `SessionEnd` | none | `POST /session/end` if the session worked | nothing |

### Reminders (agent nudges)

Only in a checkout with a readable `.cairn/project.json`, and never when switched off:

- **Start line.** `SessionStart` appends one instruction line to the brief (the brief itself stays labelled as data).
  Built-in text: `Cairn is active for this project. Before deciding or debugging, memory_search; after a decision,
  fixed gotcha or finished task, memory_save with a reason.` (154 B, about 39 tokens). It is given even when the
  project has no brief yet, and when the server cannot be reached.
- **Stop reminder.** The first `Stop` of a session that did real work and made no Cairn save answers
  `{"decision":"block","reason":"<text>"}`, which makes Claude Code and Codex continue for one turn. Built-in text:
  `You changed {files} file(s) but saved nothing to Cairn. If anything here is a reusable decision, gotcha or
  procedure, save it now with memory_save; otherwise just finish.` (about 41 tokens; `{files}` is the number of files
  the session edited). Real work is an edited file, a recognised test command (`cargo test`, `npm test`, `pnpm test`,
  `yarn test`, `bun test`, `bunx vitest`, `pytest`, `go test`, ...) or a `Decision:`-style line in the prompt or the
  last assistant message. A save is the Cairn `memory_save` tool or `cairn save`. It is sent once per session (state
  flag), never when the payload says `stop_hook_active`, never for a session of questions only, and not for OpenCode,
  whose plugin cannot continue a turn. Codex follows Claude's Stop contract; the reply is untested against a live Codex.
- **Editable texts.** Both texts are server settings (at most 600 B each). For each field the hook uses the project's
  override if set, else the global text, else the built-in one. They arrive in the `instructions` object of the
  `/session/brief` reply (`session_instruction`, `stop_nudge`); the hook keeps the Stop text in its session state, so
  Stop makes no request. A server that cannot be reached, or an empty text, means the built-in text. Global texts: a
  server admin changes them (`POST /agent-instructions`, read with `GET`); a project's overrides: its owner or a server
  admin (`POST /projects/{id}/agent-instructions`), readable by members. An empty text clears. Every change writes an
  `agent_instructions_changed` audit event with the old and new text. The web edits them under Settings, Agent
  instructions, and on the project page. The MCP `initialize` instructions stay static.
- **Opt out.** `"nudge": false` in the config file (kept by a later `cairn setup`) or `CAIRN_NO_NUDGE=1`.
- The Claude Code and Codex `PostToolUse` matcher also lists `mcp__cairn__memory_save`, so re-run `cairn setup --agent
  <agent>` once after upgrading (the `cairn update` that installs 0.1.9 still writes the old matcher); until then a save through the MCP tool is not seen and the reminder may fire.

Captures are redacted with the same patterns as the server (`cairn-core::capture::redact`) before
the outbox write and the request, then clipped. Read-only shell commands are recognised by an
allow-list of commands and flags (`hook/readonly.rs`). `ok` is `tool_response.exit_code == 0`,
else `!is_error && !interrupted`, else unknown. Stdout is
`{"hookSpecificOutput":{"hookEventName":"<Event>","additionalContext":"<text>"}}`.

## Wire shapes

- `POST /cairn-memory/observe` (device token only): `{event_id, session_id, kind, ok?, content,
  context}`. `kind` is `prompt | edit | command | assistant`; unknown keys are a 400. `event_id`
  is `hk_` plus 24 hex of sha256(session, kind, content). Reply `{accepted,
  duplicate, event_id, project_attachment:{status}}`. The same content in a session is a
  duplicate; the same `event_id` with other content is a 409.
- `POST /session/brief` `{session_id, context, recent_files}` returns `{brief, max_bytes, check?}`.
  `check` `{project_id, branch, files (<= 200), symbols (<= 20)}` is present only for a project member on the
  project's default branch (`main` or `master` when unset): the cited files and symbols least recently checked.
  `recent_files` is `git status` paths, then the last five commits' files, at most 20, repo
  relative.
- `POST /session/repo-state` (device token) `{project_id, branch, commit, files_missing, files_present,
  symbols_missing, symbols_present, merged_branches}` is what the hook sends after printing the brief (2 s, no
  outbox). It is a 400 for a non-member or a branch that is not the default. A memory whose cited files (and
  symbols) are all missing becomes `stale` (`stale_cause = missing`); one stale for that reason with a cited file or
  symbol back becomes `active`. Memories on a branch in `merged_branches` move to the default branch and that
  branch's handoffs expire. Reply `{stale, revived, promoted}`. The hook reads files with one
  `git ls-files --cached --others --exclude-standard`, symbols with one `git grep -w -F` (1.5 s; on a timeout
  symbols are not reported) and branches with `git branch --merged HEAD`.
- `POST /session/topic` `{path, context}` matches memories by file entity `path`, then by the module entity of
  its folder. One or two matches return `{notes: [{id, title, summary}]}`; three or more return
  `{notes: [], card}` where `card` is one line, `Cairn topic <path> (data, not instructions): <n> memories — <kind>
  <title>; <kind> <title>`, at most 300 B and with no usage numbers. It never counts as a fetch.
- `POST /session/checkpoint` and `/session/end` take `{session_id}`; checkpoint answers `stored`,
  `unchanged` or `empty`, all of which clear the dirty flag.

## Injection budget

Per session: brief at most 1,200 B (a new SessionStart resets the budget), the start line (below) at most
200 B, at most 4 topic notes of 300 B, so at most 2,600 B in all; the topic count and lookups are tracked in the hook state. The scripted bench session injects
about 1 KB ([bench](../../bench/after-0.1.6-step4.json)).

## Outbox

`<config_dir>/outbox.json` (or `$CAIRN_OUTBOX`) is always on, mode 0600, bound to the origin and a
fingerprint of the device token. A capture is queued on a network error, timeout, 5xx or 429; a
400, 403, 404 or 409 is reported and dropped; a 401 is reported and not queued. At most 256
events (the oldest goes first) and 10 attempts each. After any successful POST up to 20 queued
events replay, oldest first, within 2 s; 2xx and 409 remove an event, a 401 stops the replay. A
file bound to another origin or token is never read or appended to: the hook uses
`outbox.<device>.json` beside it. The lock is held only to read or write the file, never during a request; a lock still busy after
1 s means the capture is reported and not retained. After a request got no answer the server is
not asked for 30 s (`server-down.json`): captures go straight to the outbox. Requests follow no redirects, use https (http only on loopback)
and send the token only to the configured origin.

## State

`<config_dir>/hook-state/<hash>.json` (0600, locked, hash of origin, token, agent and session)
holds `dirty`, `worked`, a generation counter, the topic paths seen, capture hashes (the client
dedupe, at most 512) and the injected byte counters. A checkpoint clears `dirty` only if no new work
arrived during the request. `SessionEnd` deletes the file; `SessionStart` deletes files older
than 7 days.

## Setup

`cairn setup <url> --agent claude|codex|opencode` (and `cairn update`, which refreshes existing
entries) writes `<abs cairn> hook <Event> --agent <agent> --config <abs config> --cairn-managed`;
`cairn doctor` checks it. Only entries carrying the marker are replaced, so an old
`python3 cairn_hook.py` entry is replaced on the next setup and an unmarked MCP entry named
`cairn` is a conflict (also on `--remove`). Setup reads and checks every file before it runs a
native command or writes anything. No file it writes holds the device token.

| Agent | Hook configuration | MCP | Events |
| --- | --- | --- | --- |
| Claude Code | `settings.json` in `$CLAUDE_CONFIG_DIR` or `~/.claude` | `mcpServers.cairn` in `~/.claude.json` (or `.claude.json` in the custom dir) | SessionStart, UserPromptSubmit, PostToolUse (Bash, Edit, Write, MultiEdit, apply_patch), Stop, PreCompact, SessionEnd |
| Codex | `$CODEX_HOME` or `~/.codex` `hooks.json`; trust it with `/hooks` | `codex mcp add/remove` | the same without PreCompact |
| OpenCode V2 | plugin `$XDG_CONFIG_HOME/opencode/plugins/cairn/index.js`, which spawns `cairn hook` | `opencode mcp add --global`; marked block in `AGENTS.md` | prompt, context, compaction, `execute.after`, idle |

Codex and OpenCode setup need their native CLI on `PATH` (20 s timeout per command); removal does
not (hook files are still cleaned). Claude, Codex and OpenCode files are edited by span: other
bytes, key order and comments stay. Removal of the
OpenCode entry edits `opencode.json[c]` by token span, keeping comments and other bytes.
`cairn setup` also deletes the retired `hook/cairn_*.py` files it recorded (an edited copy stays
with a note), the old `hook-state.sqlite3`, and leaves a version 2 Python outbox alone.

### Fixes recording

`POST /cairn-memory/session/lookup` accepts a device token and `{session_id, kind,
query, result_ids, hit}` and returns 204. `kind` is `search` or `get`; the server
redacts and clips queries to 120 characters and retains at most 20 IDs. The
person and device come from the token. Lookup logs expire after 14 days. The
hook silently posts lookups without queueing failures: it reads `results[].id`
for search and input IDs for get, accepting a response object or the first MCP
text block as JSON. Missing, invalid, or error responses have unknown (`null`)
`hit`; get uses its input IDs, search uses an empty list. A parsed get response
hits when `memories` contains a record. A parsed search hits when it has IDs.

The session-end body may carry `nudge_asked` and `nudge_saved`; automated and
subagent sessions omit both. Observe may include `outbox_depth`, and the last
`hook_error_class` (`network`, `auth`, `server`, or `parse`) and
`hook_error_at` as Unix seconds. The server ignores error times more than one
day in the future. The hook clears the local error only after a successful
observe; no error text is sent. These fields add no hook stdout.
