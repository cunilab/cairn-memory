---
id: CM-CONTRACT-005
title: "Layer 1 Hook Capture Adapter"
status: draft
---

# Layer 1 Hook Capture Adapter

`scripts/cairn_hook.py` is the portable hook boundary for local integrations.
It lets an agent hook send one JSON object without knowing Cairn's internal
storage or lifecycle rules.

## Invocation

```bash
printf '%s\n' '{"sessionId":"session-1","content":"The test passed.","role":"tool","source":"PostToolUse"}' \
  | CAIRN_DEVICE_TOKEN='<device-token>' CAIRN_URL=http://127.0.0.1:3111 python3 scripts/cairn_hook.py
```

`CAIRN_URL` defaults to `http://127.0.0.1:3111`. It must use `https`, or
`http` only for a loopback host (`localhost`, `127.0.0.0/8`, `::1`), the same
rule as the `cairn` CLI. Userinfo, a query, a fragment, or another scheme is
refused. A path is allowed as a base for a reverse proxy mount, for example
`CAIRN_URL=https://cairn.example.test/agent`; the hook posts to
`<base>/cairn-memory/sync/push`. Otherwise the hook exits before it reads stdin and before it connects,
with a fixed message that does not include the URL, so the device token never
travels over plain http beyond the host. Plain-http loopback requests ignore
`HTTP_PROXY`. Without a queue binding, the adapter then only strips trailing
slashes from the URL and keeps a path. `normalize_origin` (which also refuses a
path) applies only when `CAIRN_OUTBOX` is set. If `eventId` is absent, the
adapter derives a stable fingerprint from the session, source, and content so
retries remain idempotent. If `sessionId` is absent, `CAIRN_SESSION_ID` is
required.

`CAIRN_DEVICE_TOKEN` is required. The value is the raw token, not a `Bearer`
prefix, and it is an environment variable rather than a command argument.
Create the device in the web console (Devices page); the token is shown once.
Without it the hook exits before it reads stdin and posts nothing. The hook
no longer reads `CAIRN_ACCOUNT_TOKEN`: a value there is ignored and never sent.

```bash
CAIRN_DEVICE_TOKEN='<device-token>' CAIRN_URL=http://127.0.0.1:3111 \
  python3 scripts/cairn_hook.py

# Opt-in device queue. Unset CAIRN_OUTBOX keeps the one-shot delivery above.
CAIRN_OUTBOX=/path/to/device-queue.json \
  CAIRN_DEVICE_TOKEN='<device-token>' CAIRN_URL=http://127.0.0.1:3111 \
  python3 scripts/cairn_hook.py
```

Generic capture posts `POST /cairn-memory/sync/push`, which is the capture
route on both the file store and PostgreSQL. This adapter does not register a
device or start a session. A device token also starts and ends sessions (the
server accepts it on every agent route); see the
[authentication contract](auth-layer3.md#device-tokens-are-agent-credentials).

## Normalized fields

The adapter accepts top-level or nested `payload` values for:

- `eventId` / `event_id`
- `sessionId` / `session_id`
- `content`, `text`, or `message`
- `role`
- `source`, `hook`, or `type` (top level), and `source` or `type` (nested)
- `importance`
- `context`
- `lifecycle`

If none of `content`, `text`, or `message` is present, the whole input object
becomes the content as compact sorted JSON. `role` defaults to `tool` and the
source defaults to `hook`.

## Capture hygiene (v0.1.4)

Automatic assistant text is skipped by default: explicit `role: assistant`
(top level or nested payload), the presence of `last_assistant_message` at either
level, and Claude Code `Stop` events produce no HTTP
request and do not create or touch an outbox. Set
`CAIRN_CAPTURE_ASSISTANT=1` on the hook process to opt in. Other values keep
the default. Explicit CLI/MCP saves and captures remain agent-directed.

A regular file named `.cairnignore` skips hook capture for that folder and its
descendants. Lookup starts at the resolved payload `cwd` (top level, then
nested payload), or the process working directory if absent. Within Git it
checks each ancestor through the Git toplevel, inclusive, and stops there;
outside Git it checks through the filesystem root. A marker directory does
not count. A skip performs no network or outbox/lock writes. Existing queued
events are not purged. Credential and URL validation still run before stdin.

Secret scrubbing runs on the complete content before either the 16000-character
tool cut or the 64-KiB content cut, and before event fingerprinting. The outbox
and server also retain their redaction boundaries. A generated fingerprint
uses the scrubbed, bounded content, so captures differing only in scrubbed
secret values can coalesce. Explicit event IDs are unchanged. Redaction is
best-effort; markers are the way to exclude a sensitive folder altogether.

## Claude Code hook fields

Claude Code runs a hook command with one JSON object on stdin
([hooks reference](https://code.claude.com/docs/en/hooks)). The adapter reads
these fields directly, so a settings entry needs no wrapper script. Setup is in
[`examples/README.md`](../../examples/README.md).

| Claude Code field | Cairn field | Rule |
| --- | --- | --- |
| `session_id` | `sessionId` | Same as the `session_id` alias above. |
| `hook_event_name` | `source` | First choice for `source` (before `source`, `hook`, `type`), so a `SessionStart` input with `source: "startup"` is still recorded as `SessionStart`. |
| `prompt` (`UserPromptSubmit`) | `content` | After `content`, `text`, `message`. |
| `last_assistant_message` (`Stop`) | `content` | After `prompt`, only with assistant capture opted in. |
| `tool_name`, `tool_input`, `tool_response` (`PostToolUse`) | `content` | With no field above: `<tool_name> {"tool_input":...,"tool_response":...}` as compact sorted JSON, cut to 16000 characters. |
| other events, e.g. `SessionStart`, `SessionEnd` | `content` | Whole input object as compact sorted JSON (the fallback above). |
| `hook_event_name` | `role` | `UserPromptSubmit` is `user`, `Stop` is `assistant`, anything else is `tool`. An explicit `role` wins. |

Claude Code sends no repository or branch in its hook input, only `cwd`. When
the payload has no `context.repository` (top level or nested), the hook derives
one from `cwd` with at most three `git -C <cwd>` calls (no shell, 1 second
each; any failure leaves that part empty):

| Context field | Source |
| --- | --- |
| `repository` | SSH/HTTPS origin normalized to `host/org/repo`, host lowercase and `.git` stripped; without a network remote use `local:<top-level directory>`. Credentials are never part of the locator. |
| `branch` | `rev-parse --abbrev-ref HEAD`; a detached HEAD sends none. |

`cwd` that is missing, not a repository, or a machine without `git` leaves
`context` empty as before. Derived values fill only what the payload lacks: an
explicit `context.repository` is never replaced, and other keys of a partial
`context` are kept. The context is not part of the event fingerprint, so ids
do not change. Hosts and organizations distinguish repositories with the same short name. A repository tag alone creates no project.

The hook also reads the committed `.cairn/project.json`
([project-layer4](project-layer4.md#init-and-membership)) at the git
top level of the input's `cwd` (one more `git -C <cwd> rev-parse --show-toplevel`, then a file
read) and adds its `project_id` as `context.project_id`. That works with or without an explicit
`context.repository`, so a hook payload can carry both. An explicit `context.project_id` is
never replaced. A missing file, a `cwd` outside a repository or missing `git` adds no project.
An existing broken, unreadable or invalid file warns on stderr and capture remains
personal. The hook and CLI include local file warnings in the returned
`project_attachment.warning` as well as stderr. The server joins a known project ID, or returns `project_attachment` with
a warning for failed/unknown attachment and keeps the capture personal. It retries
attachment on the next capture, including a duplicate replay, without creating a
fallback project. The ID is not part of the event fingerprint. Since 0.1.4, init also writes the
project display `name` alongside `project_id` and `repository`. Hook and CLI capture
readers accept files with or without `name`; only the ID is sent as capture
context. Names do not change membership or fingerprints.

Claude Code sends no event id, so the id is the derived fingerprint: the same
session, source, and content is one event (an identical repeated prompt in a
session is stored once). When the input has a `tool_use_id`, it is part of the
fingerprint, so two identical tool results in one session stay separate events.
Content over 64 KiB (a very long prompt or reply) is cut at a character
boundary and ends with `[truncated]` instead of being rejected, so the capture
is kept. Generic capture prints the server's JSON reply on stdout. Configured agent mode
prints only instruction/context JSON; its stdout must reach the host for
injection, as shown in the current example.

The adapter posts one request and does not follow a redirect. It does not
start a background process, a thread, or a second POST of its own event. With `CAIRN_OUTBOX`
unset, a failed delivery is still dropped.

`CAIRN_OUTBOX` is an opt-in setting. The hook validates
`normalize_origin(CAIRN_URL)` before it reads stdin and before it connects.
A `%` in the hostname, userinfo, a query, a fragment, or a path that holds an empty, dot, `%`, or `cairn-memory` segment is rejected
with the fixed origin message, and plain http to a non-loopback host with the
loopback message. That message does not include the URL. The
canonical base (origin plus path prefix) is both the request base and the queue binding. There is no
separate URL-parser split.

Before the POST, an existing queue is checked under `LOCK_EX|LOCK_NB`. A
version 1 file, another origin, another route, another device fingerprint, or
invalid metadata fails closed. Those bytes are not rewritten and are not
purged. A missing file is not created by that check. The sibling lock file
may be. The lock is released before the socket call. The append takes the
same nonblocking lock and checks the binding again. CLI `enqueue`, `replay`,
and `purge` still wait. The hook does not.

A retryable device failure is `URLError` (connection refused or disconnect),
`TimeoutError`, `ConnectionError`, any other `OSError` while connecting or
reading, a malformed or incomplete HTTP response, or HTTP 408, 429, or 5xx. The hook appends the normalized body once, with the
same event id, and exits 0 only after that save or when that id is already in
this bound file. Stderr is `Cairn hook queued for replay`. It does not
include the token, the fingerprint, the raw content, or the URL.

HTTP 401, other 4xx, and every 3xx are not queued. A 2xx is not queued. The
hook does not purge. Invalid input and invalid configuration are not queued.
Explicit replay remains
[`scripts/cairn_outbox.py`](../../scripts/cairn_outbox.py) and does not read
`CAIRN_DEVICE_TOKEN`; the hook's pass takes the token from its own
environment. A matching 401 purges only inside that explicit replay,
after the version 2 origin, route, and fingerprint matched.

## Opportunistic replay

With `CAIRN_OUTBOX` and `CAIRN_DEVICE_TOKEN`, after the hook's own event
returns 2xx the hook takes the queue lock without waiting and, if it gets it,
replays the oldest pending events for the same origin and device. At most 20
events and a 2 second wall-clock budget: each request timeout is the budget
that is left, so no request outlives it. The first transport failure, bad
acknowledgement, 401, 408, 429, or 5xx stops the pass. The pass never purges:
a 401 stops it, adds an attempt, and leaves every event queued. Only explicit
`replay` purges. Events that already have 3 failed attempts are skipped and do
not count against the 20, so they cannot starve newer ones; explicit `replay`
still retries them. A 409 (the server holds different content
under that event id) is not retried: the event leaves the queue and the queue
file's `rejectedCount` and `lastRejected` record it, which
`cairn_outbox.py status` prints. The pass skips silently on a busy lock, and a failure in
it never changes the hook's exit status or output. The hook's own event is
not sent twice, and the hook starts no background process or thread. It uses
the replay code of `cairn_outbox.py`, so the binding check, redirect block,
and attempt counters are the explicit `replay` rules in the
[sync contract](./sync-layer3.md#opportunistic-replay). A queue that is not
bound to this origin and token fails the preflight before any POST. A busy
lock at preflight is not fatal: the hook still posts, and the append rechecks
the binding. Only if that post then needs a retry does the busy lock make the
run exit nonzero with the capture not retained, as below.

A full queue, a disk error, a binding or version clash, or a busy lock at
the retry append exits nonzero and leaves the previous bytes unchanged. A failed create leaves no
queue file and no payload-bearing temporary file. A busy lock is a caller
failure. The capture is not retained until a later hook run saves it. A
nonzero exit is not a saved event. A caller that ignores it can drop the
capture. The hook does not sleep, spin, or write without the lock.

An empty queued context is not the session that existed at hook time. File
and PostgreSQL copy this person's session when an accepted push context is
empty, and that copy happens when replay is accepted. A missing session stays
empty. A partial context is stored as sent. Redaction still runs before the
queue write. The event id comes from the pre-redaction session, source, and
content. There is no rebind command. A rotated device token fails the
fingerprint check and leaves the file. The old token can still purge on a
later matching 401. Raw keys outside the seven context lists, and identity
scalars, are not redacted. Older spool files are not rewritten.

| Configuration | Request | Authorization |
| --- | --- | --- |
| `CAIRN_DEVICE_TOKEN` set | `POST /cairn-memory/sync/push` | That device token. File device sync and PostgreSQL capture. |
| Unset, blank, or a token that is not one header token | No request | Configuration error. The process exits before it reads stdin. |

A blank value is unset. The process does not print the token, put it in the
URL, or store it in the payload. It does not infer a server or try another
route after a 401. A revoked token is a `401` and nothing is queued.

Cairn still performs lifecycle classification, retention, persistence, and
validation. The hook runs `cairn_outbox.normalize` before the request. That
redacts `content` and the seven `WorkContext` string lists, and it keeps the
64 KiB content limit. The server then classifies that redacted text. The event
fingerprint is still taken from the input session, source, and content before
that redaction. There is no PostgreSQL `/cairn-memory/observe` route, and the
hook does not use the file `/observe` route.

## Change record

2026-11-17: project files also contain `name`; old files without it remain readable.
`cairn init` backfills legacy files, and owner-only `cairn init --rename <name>`
repairs the server and file explicitly (see the project contract).

2026-11-16: the hook adds `context.project_id` from the committed `.cairn/project.json` at the
git top level of `cwd`; a capture that carries it makes the sender a project member.

2026-11-15: when the payload has no `context.repository`, the hook derives
`repository` and `branch` from Claude Code's `cwd` with `git`, so hook-only sessions match personal repository filters. Projects require an initialized file.

2026-11-15: a queued event refused with 409 is removed from the queue on
replay and counted (`rejectedCount`, `lastRejected`) instead of staying
`rejected` until purge; manual `replay` reports it under `rejected`. The hook
reads Claude Code's input fields (`hook_event_name`, `prompt`,
`last_assistant_message`, `tool_name`/`tool_input`/`tool_response`).

2026-11-14: the hook became device-token only. Removed: the
`CAIRN_ACCOUNT_TOKEN` observe path, the unauthenticated zero-account observe
path, and the observe-only offline refusal text. Reason: agents use device
tokens created in the web console ([ADR 0008](../adr/0008-self-hosted-account-authentication.md)).

## Agent instructions and setup (v0.1.4)

`cairn setup <server-url> --agent claude|codex|opencode` reads the device token
from stdin, saves the private Cairn config, and installs that agent's integration.
Plain URL-only setup retains its script-download behavior. Explicit agent setup
installs the scripts and instruction text embedded in the matching CLI binary,
so an unpublished build or an offline machine can configure the integration.
`cairn update` refreshes the same three release assets (`cairn_hook.py`,
`cairn_outbox.py`, `cairn_instructions.txt`) with SHA-256 verification.

`cairn setup <server-url> --agent <agent> --remove` removes only Cairn-marked
hooks, MCP registration and instruction blocks. It reads no token and makes no
Cairn request. Credentials and shared hook assets remain available to other
integrations. An unmarked MCP server named `cairn` is a conflict, including on
removal. Malformed config or instruction markers fail before agent changes.
Repeated installation/removal is idempotent. Existing unrelated settings, hooks
and MCP registrations are preserved. Python 3 is required; no Python packages
are needed. Codex and OpenCode setup/removal additionally require the respective
installed CLI. Native MCP commands have a 20-second timeout.

| Agent | Installed integration | Official capability evidence |
| --- | --- | --- |
| Claude Code | User `settings.json` hooks and user `~/.claude.json` MCP registration. `CLAUDE_CONFIG_DIR` selects the custom directory. | [Hook locations](https://code.claude.com/docs/en/hooks#hook-locations), [settings locations](https://code.claude.com/docs/en/settings#find-or-create-your-settings-files), [config directory override](https://code.claude.com/docs/en/claude-directory), [MCP user scope](https://code.claude.com/docs/en/mcp#user-scope). |
| Codex | `$CODEX_HOME/hooks.json` (default `~/.codex`); native `codex mcp list --json`, `add`, `remove` merge its TOML config. | [Hook discovery](https://learn.chatgpt.com/docs/hooks#where-codex-looks-for-hooks), [MCP CLI configuration](https://learn.chatgpt.com/docs/extend/mcp?surface=cli#configure-with-the-cli). Review/trust through `/hooks` is required by [Codex hook trust](https://learn.chatgpt.com/docs/hooks#review-and-trust-hooks); setup does not bypass it. |
| OpenCode V2 | Native `opencode mcp add --global` updates `opencode.json` or `opencode.jsonc`; compact instructions in a marked global `$XDG_CONFIG_HOME/opencode/AGENTS.md` block (default `~/.config`). | [MCP setup/local environment](https://opencode.ai/docs/mcp-servers/#setup), [V2 config](https://opencode.ai/docs/mcp-servers/#config), [instruction scope/order](https://opencode.ai/docs/instructions/#scope). |

OpenCode's documented [management interface](https://opencode.ai/docs/mcp-servers/#management)
has no MCP remove command. Cairn reads JSONC spans to check ownership and delete
only its marked entry and the required separator, preserving other bytes and
comments. Native insertion handles formatting. Duplicate keys/malformed JSONC
fail closed. Two global config files at once or a V1 `mcp.cairn` entry are
refused; migrate to one V2 config. `CAIRN_SETUP_MANAGED=1` marks the MCP process
environment; it carries no credential. Hook commands carry `--cairn-managed`;
OpenCode instruction blocks use `<!-- cairn:begin -->` / `<!-- cairn:end -->`.

### Verified agent capabilities

Verified against the official documentation on 2026-11-17; native configuration
probes used Codex 0.169.0 and OpenCode 2.1.4 with temporary homes.
The table names the relevant events; linked references own the complete event
inventories. Cairn does not claim a minimum client release for these capabilities.

| Agent | Events / context delivery | End/continue request |
| --- | --- | --- |
| Claude Code | `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `Stop`, `SessionEnd` exist in the [hook lifecycle](https://code.claude.com/docs/en/hooks#hook-lifecycle). Start and post-tool JSON `hookSpecificOutput.additionalContext` reaches the model ([context delivery](https://code.claude.com/docs/en/hooks#add-context-for-claude)). | `Stop` can continue with `decision: "block"` and `reason`; `stop_hook_active` guards recursion ([Stop control](https://code.claude.com/docs/en/hooks#stop-decision-control)). `SessionEnd` has no decision control ([decision control](https://code.claude.com/docs/en/hooks#decision-control)). |
| Codex | Same five events are documented. [SessionStart](https://learn.chatgpt.com/docs/hooks#sessionstart) and [PostToolUse](https://learn.chatgpt.com/docs/hooks#posttooluse) accept additional context. Shell/unified exec is `Bash`; patches are `apply_patch` with Edit/Write aliases ([tool coverage](https://learn.chatgpt.com/docs/hooks#tool-coverage)). | [Stop](https://learn.chatgpt.com/docs/hooks#stop) accepts blocking-decision continuation and exposes `stop_hook_active`. [SessionEnd](https://learn.chatgpt.com/docs/hooks#sessionend) is advisory and cannot keep a thread open. |
| OpenCode V2 | Plugin hooks exist: session `prompt`, `context`, `compaction`, `generate`, `title`, model/retry/fork hooks, and tool `execute.before` / `execute.after`. Context hooks can modify outgoing instructions/messages ([server plugin hooks](https://opencode.ai/docs/build/plugins/server#hooks)). | Cairn installs no plugin and uses no shell Stop continuation protocol for OpenCode. Instruction-file fallback asks the agent to recall/save appropriately. This is an adapter limit, not a claim that OpenCode lacks hooks; no automatic brief, first-work injection or guarded Stop is claimed for this adapter. |

### Hook output and work tracking

Configured Claude/Codex commands call `cairn_hook.py --agent <agent> --config
<private-config-path> --cairn-managed`. They load that config when both credential
environment variables are absent; partial overrides still fail closed. Generic
invocation without `--agent` retains its capture JSON response. Agent mode warns about a failed capture and exits successfully so the host
retains any emitted instructions (only a queued/acknowledged capture is saved).
Generic capture still exits nonzero on an unretained failure. Agent mode keeps
capture acknowledgements out of model context; only the following hook JSON is
printed. Network failure does not prevent a locally available instruction from
being injected. Existing capture/outbox behavior remains bounded and device-only.

- **SessionStart:** a single short note says Cairn exists and to use its memory
  tools, followed by the server brief. It makes one authenticated
  `POST /cairn-memory/session/brief`, never searches separately from each hook.
  An unavailable/oversized response leaves the short note and a fixed warning.
- **First real work:** the first `PostToolUse` for `Bash`, `Edit`, `Write`,
  `MultiEdit` or `apply_patch` sends the compact instructions exactly once in
  retained session state. Reads/searches do not count. Command failures still
  count as commands when the host emits PostToolUse. Resume keeps the once flag.
- **Stop:** after new real work since the last ask, returns `decision: "block"`
  and a request to save worthwhile knowledge, with the compact field/scope
  instructions. A Stop with `stop_hook_active` suppresses the request and clears
  continuation work; saving cannot create an endless Stop loop. With no new
  work there is no request. This per-turn Stop is the last point where the agent
  can act; it is not the later process/session termination event.
- **SessionEnd:** best-effort explicit rule-based distillation for a session that
  did work, then removes its delivery flags. It never asks an agent to continue.
  An unavailable close leaves ordinary idle distillation as fallback.

`scripts/cairn_instructions.txt` is the shared compact instruction source for
hooks, the OpenCode block and MCP initialization: three explicit scopes and
visibility, required title/summary/reason, known per-type fields, evidence,
deduplication, correction and when to save. The agent chooses scope and writes
reasons; no server-side LLM is invoked.

Byte bounds are UTF-8 bytes, a conservative token ceiling without a tokenizer:
`START_NOTE_MAX_BYTES=256`, `INSTRUCTIONS_MAX_BYTES=2048`,
`END_ASK_MAX_BYTES=2304`, plus `BRIEF_MAX_BYTES=4096`. The short-note budget includes its separating newline. Start output is at most
4352 bytes of context; the short note itself is one line. JSON transport escaping
is outside these model-visible text bounds. Instruction overflow fails rather
than silently dropping fields. Server brief selection and fencing are owned by
[the continuity contract](continuity-v1.md#session-start-brief-v014).

Local `hook-state.sqlite3`, next to Cairn config, stores only hashed session
bindings and instruction/work delivery flags. Hash input includes origin,
device token, agent and session ID; none is persisted in plaintext. SQLite
transactions serialize concurrent hooks. Flags persist for every open session
until its SessionEnd; they are not evicted merely because other sessions start.
This is transient delivery state, not a memory store/cache. No transcript
or memory text is persisted there. A regular `.cairnignore` skips agent hook
output, network, capture and delivery-state writes. It does not remove explicit
MCP/CLI access or the static OpenCode instruction block.
