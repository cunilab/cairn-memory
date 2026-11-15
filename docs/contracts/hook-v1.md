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
refused. Otherwise the hook exits before it reads stdin and before it connects,
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

The hook always posts `POST /cairn-memory/sync/push`, which is the capture
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

## Claude Code hook input

Claude Code runs a hook command with one JSON object on stdin
([hooks reference](https://code.claude.com/docs/en/hooks)). The adapter reads
these fields directly, so a settings entry needs no wrapper script. Setup is in
[`examples/README.md`](../../examples/README.md).

| Claude Code field | Cairn field | Rule |
| --- | --- | --- |
| `session_id` | `sessionId` | Same as the `session_id` alias above. |
| `hook_event_name` | `source` | First choice for `source` (before `source`, `hook`, `type`), so a `SessionStart` input with `source: "startup"` is still recorded as `SessionStart`. |
| `prompt` (`UserPromptSubmit`) | `content` | After `content`, `text`, `message`. |
| `last_assistant_message` (`Stop`) | `content` | After `prompt`. |
| `tool_name`, `tool_input`, `tool_response` (`PostToolUse`) | `content` | With no field above: `<tool_name> {"tool_input":...,"tool_response":...}` as compact sorted JSON, cut to 16000 characters. |
| other events, e.g. `SessionStart`, `SessionEnd` | `content` | Whole input object as compact sorted JSON (the fallback above). |
| `hook_event_name` | `role` | `UserPromptSubmit` is `user`, `Stop` is `assistant`, anything else is `tool`. An explicit `role` wins. |

Claude Code sends no repository or branch in its hook input, only `cwd`. When
the payload has no `context.repository` (top level or nested), the hook derives
one from `cwd` with at most three `git -C <cwd>` calls (no shell, 1 second
each; any failure leaves that part empty):

| Context field | Source |
| --- | --- |
| `repository` | The last path part of `remote.origin.url` without a trailing `/` or `.git` (`https://host/acme/widget.git` and `git@host:acme/widget.git` both give `widget`). With no origin, the name of the `rev-parse --show-toplevel` directory. Never a host, a user, a credential, or an absolute path. This is the short-name style `project-register` and the examples use, and it is the same on every clone, device, and worktree. |
| `branch` | `rev-parse --abbrev-ref HEAD`; a detached HEAD sends none. |

`cwd` that is missing, not a repository, or a machine without `git` leaves
`context` empty as before. Derived values fill only what the payload lacks: an
explicit `context.repository` is never replaced, and other keys of a partial
`context` are kept. The context is not part of the event fingerprint, so ids
do not change. Two repositories with the same short name under one person share
a project; send an explicit `context.repository` to tell them apart.

Claude Code sends no event id, so the id is the derived fingerprint: the same
session, source, and content is one event (an identical repeated prompt in a
session is stored once). When the input has a `tool_use_id`, it is part of the
fingerprint, so two identical tool results in one session stay separate events.
Content over 64 KiB (a very long prompt or reply) is cut at a character
boundary and ends with `[truncated]` instead of being rejected, so the capture
is kept. The hook prints the server's JSON reply on stdout, so an
example command redirects stdout to `/dev/null` instead of letting
`UserPromptSubmit` or `SessionStart` add it to the model's context.

The adapter posts one request and does not follow a redirect. It does not
start a background process, a thread, or a second POST of its own event. With `CAIRN_OUTBOX`
unset, a failed delivery is still dropped.

`CAIRN_OUTBOX` is an opt-in setting. The hook validates
`normalize_origin(CAIRN_URL)` before it reads stdin and before it connects.
A `%` in the hostname, userinfo, a query, a fragment, or a path is rejected
with the fixed origin message, and plain http to a non-loopback host with the
loopback message. That message does not include the URL. The
canonical origin is both the request base and the queue binding. There is no
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

2026-11-15: when the payload has no `context.repository`, the hook derives
`repository` and `branch` from Claude Code's `cwd` with `git`, so hook-only
sessions register a project and branch and match repository filters.

2026-11-15: a queued event refused with 409 is removed from the queue on
replay and counted (`rejectedCount`, `lastRejected`) instead of staying
`rejected` until purge; manual `replay` reports it under `rejected`. The hook
reads Claude Code's input fields (`hook_event_name`, `prompt`,
`last_assistant_message`, `tool_name`/`tool_input`/`tool_response`).

2026-11-14: the hook became device-token only. Removed: the
`CAIRN_ACCOUNT_TOKEN` observe path, the unauthenticated zero-account observe
path, and the observe-only offline refusal text. Reason: agents use device
tokens created in the web console ([ADR 0008](../adr/0008-self-hosted-account-authentication.md)).
