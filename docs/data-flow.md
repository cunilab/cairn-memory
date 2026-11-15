---
id: CM-GUIDE-001
title: "Cairn Memory Data Flow Guide"
status: draft
---

# Cairn Memory Data Flow Guide

A picture-first explainer of how Cairn memory works today, for someone who does not read the code. It describes what the code does as of 2026-11-15; the diagrams were read from the code, not produced by running it. Behavior rules live in their owners: [architecture](./architecture.md), [policy](./policy.md), and the [contracts](./contracts/). If this guide and an owner disagree, the owner wins and this guide needs a fix.

**How to read the diagrams.** A solid arrow always happens. A dashed arrow is optional or conditional. Anything not built is marked "not implemented" in the text, not drawn.

**Two stores.** The server keeps data in either a JSON file (`--store`) or PostgreSQL (`--database-url`). The Docker dev stack uses PostgreSQL. Both expose the same routes. Differences that matter are listed at the end of the capture section.

**Two credentials.** A *web session* is what a person gets by signing in to the console. A *device token* is what an AI agent uses; only a web session can create one. Section 7 explains the split, including the one admin account that creates all other accounts.

| Word | Meaning |
| --- | --- |
| Observation | One captured event, such as a tool result. Raw and temporary. |
| Session | A run of work; observations belong to one. |
| Memory | Something Cairn keeps: a session summary, a saved fact or lesson, a correction, or a project decision. |
| Project | A repository registered under a person, with branches and optional members. |
| Decision | A memory copied into a project (the result of promotion). |

## 1. System map

Two kinds of users reach one server. People use the web console, which holds their sign-in as an httpOnly cookie and forwards it as a bearer token. Agents use a device token through the hook, the `cairn` command, or `cairn mcp`. The hook reads its URL and token only from environment variables; the CLI and MCP read a saved config file or the same two variables. The URL may include a path prefix when a reverse proxy mounts the API under one (for example `https://cairn.example.test/agent`); see [deploy](./deploy.md).

```mermaid
flowchart LR
    Owner(["Owner or member<br/>in a browser"])
    subgraph Console["Next.js console (apps/web)"]
        Pages["Pages: login, dashboard,<br/>projects, devices,<br/>accounts (admin only)"]
        Proxy["Proxy /api/cairn/*<br/>cookie cairn_token becomes Bearer"]
        Pages --> Proxy
    end
    subgraph Device["Agent device"]
        Host(["Agent host, for example Claude Code<br/>hooks from examples/claude-code-hooks.json"])
        Hook["scripts/cairn_hook.py"]
        Outbox[("Outbox file<br/>only if CAIRN_OUTBOX is set")]
        CLI["cairn CLI"]
        MCP["cairn mcp<br/>stdio"]
        Cfg[("config.json mode 0600<br/>or CAIRN_URL and<br/>CAIRN_DEVICE_TOKEN")]
        Host --> Hook
        Host --> CLI
        Host --> MCP
        Hook -.-> Outbox
        CLI --- Cfg
        MCP --- Cfg
    end
    Server["cairn-server<br/>REST /cairn-memory/*"]
    Viewer["/viewer<br/>embedded page"]
    FileStore[("File store<br/>JSON file")]
    PG[("PostgreSQL")]
    Owner --> Pages
    Proxy -->|"web session token"| Server
    Hook -->|"device token<br/>POST sync/push only"| Server
    CLI -->|"device token"| Server
    MCP -->|"device token"| Server
    Server --- Viewer
    Server -->|"chosen at start"| FileStore
    Server -->|"chosen at start"| PG
```

Where in the code: `apps/web/app/api/cairn/[...path]/route.ts` (proxy, cookie), `crates/server/src/main.rs` (store choice, idle sweep timer), `crates/client/src/` (CLI, MCP, config), `scripts/cairn_hook.py`, `scripts/cairn_outbox.py`, `examples/claude-code-hooks.json`.

## 2. Memory lifecycle

Everything starts as a raw observation. Secrets are scrubbed first and the sensitivity label can only be raised, never lowered. When a session has had no new capture for 30 minutes, it is turned into one durable summary memory. That happens on the same person's next push or session start, or at the latest on the server's 5-minute timer, so a session closes even when the agent just stops. People can correct (replace but keep history) or forget (hide but keep the record).

```mermaid
flowchart TD
    A["Event captured<br/>hook, cairn capture, memory_observe"] --> B["Secrets redacted in the text and<br/>the 7 context lists"]
    B --> C["Sensitivity label at least what the text shows<br/>public, private, sensitive, secret"]
    C --> D[("Observation stored<br/>temporary, expires after 30 days by default")]
    D --> E["Belongs to a session"]
    E --> F{"No new capture in this session<br/>for 30 minutes?"}
    F -->|"yes, and that person pushes or<br/>starts a session"| G["Session summary memory<br/>durable, with next steps<br/>and unresolved work"]
    F -->|"yes, and the 5-minute<br/>server timer runs"| G
    E -->|"cairn end"| G
    X["cairn lesson or memory_save"] --> H
    G --> H["Recall<br/>search, context, resume,<br/>unresolved, why-changed"]
    D -->|"expiry time passes"| Z["Expired<br/>left out of recall"]
    H -->|"cairn correct"| K["Correction memory, confidence 100<br/>supersedes the old one"]
    K --> M["Old memory stays stored and linked<br/>hidden from normal recall"]
    K --> H
    H -->|"cairn forget or delete"| T["Tombstone<br/>hidden from recall, record kept<br/>in storage and export"]
    T -.->|"a forgotten source observation"| G
```

The last dashed arrow means: if a source observation of a summary is forgotten, the next sweep or explicit end rebuilds the summary from what remains. A summary the person forgot or corrected is never rebuilt. A forget also leaves a deletion instruction in the person's sync stream; section 8 explains why no shipped client needs to read it.

Where in the code: `crates/core/src/lib.rs` (`finish_idle_sessions`, `sweep_idle_sessions`, `Lifecycle::observation`, `correct_for_account`, `hide_id`, `summary_is_stale`, `build_session_digest`), `crates/storage/src/lib.rs` (`IDLE_SESSION_SECONDS`, `finish_idle_sessions`, `sweep_idle_sessions`), `crates/server/src/main.rs` (`IDLE_SWEEP_INTERVAL`), `migrations/0012_session_summary_pending.sql`. Rules: [lifecycle](./contracts/lifecycle-v1.md), [continuity](./contracts/continuity-v1.md).

### 2.1 What each Claude Code hook sends

`examples/claude-code-hooks.json` runs `scripts/cairn_hook.py` for four Claude Code hook events. Claude Code passes one JSON object on standard input; the hook turns it into one `sync/push` body. Every body has the same shape: `eventId`, `sessionId`, `content`, `role`, `source`, plus a `context` (section 2.1) and an empty `lifecycle`.

| Event | When Claude Code fires it | Fields the hook reads | `content` | `role` | `source` |
| --- | --- | --- | --- | --- | --- |
| `UserPromptSubmit` | The user submits a prompt, before Claude works on it | `session_id`, `hook_event_name`, `prompt` | The prompt text | `user` | `UserPromptSubmit` |
| `PostToolUse`, matcher `Bash\|Edit\|Write\|MultiEdit` | One of those four tools has finished | `session_id`, `hook_event_name`, `tool_name`, `tool_input`, `tool_response`, `tool_use_id` | `<tool_name> {"tool_input":...,"tool_response":...}` as compact sorted JSON, cut to 16000 characters | `tool` | `PostToolUse` |
| `Stop` | Claude finishes answering a turn | `session_id`, `hook_event_name`, `last_assistant_message` | The final reply of the turn | `assistant` | `Stop` |
| `SessionEnd` | The Claude Code session ends | `session_id`, `hook_event_name`, and every other field | The whole input object as compact sorted JSON | `tool` | `SessionEnd` |

Claude Code also sends fields the hook does not copy into the content, such as `cwd`, `transcript_path`, and `permission_mode`; they matter for the content only on `SessionEnd`, where the whole object (including `cwd` and `transcript_path`) becomes the content. The hook does read `cwd`, for `context` (below). When a field the hook looks for is missing (for example `Stop` without `last_assistant_message`), the whole input object becomes the content instead. When Claude Code fires each event follows its hooks reference and was not checked in this repository.

How the hook builds the rest of the body:

- `sessionId` is Claude Code's `session_id`. Without one the hook needs `CAIRN_SESSION_ID`, or it exits.
- `eventId` is `hook-` plus the first 24 hex characters of a SHA-256 over the JSON of `sessionId`, `source`, `content`, and, for `PostToolUse`, `tool_use_id`. It is computed before redaction. Claude Code sends no event id of its own. Consequence: an identical prompt or reply repeated in one session is the same event and is stored once; two identical tool results stay separate because their `tool_use_id` differs.
- `content` over 64 KiB is cut at a character boundary and ends with `[truncated]`, so the capture is kept instead of rejected.
- Before sending, the hook redacts secrets in `content` (and in the 7 context lists, which the hook leaves empty). The server redacts again and sets the sensitivity floor.
- `context` holds a repository and branch derived from `cwd`, because Claude Code's input carries no repository or branch field. The hook runs `git -C <cwd>` (at most three calls, no shell, 1 second each): `repository` is the last path part of `remote.origin.url` without `.git` (for example `widget` from `https://host/acme/widget.git`), or the top-level directory name when there is no origin; `branch` is the current branch, and a detached HEAD sends none. A `cwd` that is not a git repository, or a machine without `git`, leaves `context` empty. An explicit `context.repository` in the input is never replaced. The event id does not depend on the context. Section 2.5 lists what this does not fix.

What the hook produces for one run of each event (`session_id` `cc-7f3a`; these values come from running the hook's request builder, without a server):

| Event | Claude Code input (shortened) | `eventId` | `content` stored |
| --- | --- | --- | --- |
| `UserPromptSubmit` | `{"prompt":"fix the login bug in auth.rs", ...}` | `hook-3e2f0bb34c5230a4cfaa4a82` | `fix the login bug in auth.rs` |
| `PostToolUse` | `{"tool_name":"Bash","tool_use_id":"toolu_01A","tool_input":{"command":"cargo test"},"tool_response":{"stdout":"test result: ok","stderr":""}}` | `hook-770b1771fc1e5de19a7725fa` | `Bash {"tool_input":{"command":"cargo test"},"tool_response":{"stderr":"","stdout":"test result: ok"}}` |
| `Stop` | `{"last_assistant_message":"Fixed. The test passes.", ...}` | `hook-6cb560963b2d2af1bdc9a165` | `Fixed. The test passes.` |
| `SessionEnd` | `{"reason":"other","cwd":"/work/app","transcript_path":"/t.jsonl", ...}` | `hook-19f20cf5c1b062056dd8c714` | `{"cwd":"/work/app","hook_event_name":"SessionEnd","reason":"other","session_id":"cc-7f3a","transcript_path":"/t.jsonl"}` |

The server turns each of these into an observation: the same text after redaction, a sensitivity label, a temporary retention (30 days by default), the person who owns the device, and a sync cursor. After a 30-minute pause, those four become one session summary memory along these lines (shortened):

```text
Session summary (deterministically compressed):
- fix the login bug in auth.rs [evidence: hook-3e2f0bb34c5230a4cfaa4a82]
- Bash {"tool_input":{"command":"cargo test"}, ...} [evidence: hook-770b1771fc1e5de19a7725fa]
- Fixed. The test passes. [evidence: hook-6cb560963b2d2af1bdc9a165]
- {"cwd":"/work/app","hook_event_name":"SessionEnd", ...} [evidence: hook-19f20cf5c1b062056dd8c714]
Source actors:
- UserPromptSubmit
- PostToolUse
- Stop
- SessionEnd
```

The summary lists repeated lines once, notes how many it left out, and pulls lines containing words such as "next step", "todo", or "blocked" into its next-steps and unresolved-work lists.

Where in the code: `scripts/cairn_hook.py` (`build_request`, `git_context`, `HOST_ROLES`, `TOOL_CONTENT_CHARS`), `scripts/cairn_outbox.py` (`normalize`), `crates/core/src/lib.rs` (`build_session_digest`, `derive_session_memory`). Rules: [hook](./contracts/hook-v1.md#claude-code-hook-input).

### 2.2 One prompt, from keystroke to later recall

This is the whole path for one event, including the failure branch and the two ways a session closes. Steps 1 to 15 happen within a second or two (the replay in steps 11 to 14 is capped at about 2 seconds); steps 16 to 21 happen later.

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant CC as Claude Code
    participant Hook as cairn_hook.py
    participant Q as Outbox file
    participant Srv as cairn-server
    participant Db as Store
    participant Tm as 5-minute timer
    participant Ag as Agent via CLI or MCP

    User->>CC: types a prompt and submits it
    CC->>Hook: UserPromptSubmit, one JSON object on stdin
    Hook->>Hook: build content, role, source, sessionId and eventId
    Hook->>Hook: cut content over 64 KiB, redact secrets
    Hook->>Srv: POST sync/push with the device token
    Srv->>Srv: check token, validate, redact again, set sensitivity floor
    alt server reachable
        Srv->>Db: store observation, owner and sync event, create the session row if new
        Srv->>Db: register project and branch if context has a repository
        Srv->>Db: close this person's sessions idle over 30 minutes, max 16
        Srv-->>Hook: accepted, cursor
        opt CAIRN_OUTBOX set
            Hook->>Q: read oldest queued events, skip any with 3 failed attempts
            Hook->>Srv: replay up to 20 events in about 2 s
            Srv-->>Hook: accepted per event
            Hook->>Q: remove delivered events
        end
    else outage, timeout, 408, 429 or 5xx, with CAIRN_OUTBOX set
        Hook->>Q: append the same body, max 256 queued
        Note over Hook,Q: stderr says queued for replay, exit code 0
    end
    Note over CC,Srv: more prompts, tool calls and replies repeat steps 2 to 15
    Tm->>Srv: every 5 minutes, close idle sessions
    Srv->>Db: for each session with no capture for 30 minutes, max 16 per run
    Db-->>Srv: session summary memory stored, session ended
    Ag->>Srv: context, search or resume with the device token
    Srv->>Db: that person's memories only, ranked
    Srv-->>Ag: matches, including the session summary
```

Notes on the steps:

- Step 5 sends at most one request per event and never follows a redirect. A `401`, a `409`, or any other `4xx` is not queued: the hook exits with an error and the event is lost (a `409` means the same event id already holds different content).
- Without `CAIRN_OUTBOX`, any failed delivery exits with an error and the event is lost.
- A full queue, a disk error, or a busy queue lock when the hook needs to append also exits with an error and the capture is not kept.
- The replay in steps 11 to 14 runs only after the hook's own event was accepted, it never purges, and a failure in it never changes the hook's exit code. The queued events themselves go through the same server checks and writes as steps 6 to 9.
- The session row is created by the push itself on both stores (the hook never starts a session). On PostgreSQL the push also marks the session pending with the time of the capture; the 30-minute clock counts from the newest capture.
- The `SessionEnd` event does not end the Cairn session. It is one more observation and restarts the 30-minute clock. The session closes through the idle rule, or earlier with `cairn end` or `memory_session_end`.

### 2.3 A session's life

A session is the unit that gets summarised. Its id is Claude Code's `session_id`. "Idle" is not stored; it is simply 30 minutes without a capture.

```mermaid
stateDiagram-v2
    [*] --> Open: first push creates the row, or cairn start
    Open --> Open: a new capture restarts the 30-minute clock
    Open --> Idle: no capture for 30 minutes
    Idle --> Open: a capture arrives before any sweep runs
    Idle --> Ended: that person pushes or starts a session, or the 5-minute timer runs
    Open --> Ended: cairn end or memory_session_end
    Ended --> Reused: a capture arrives with the same session id
    Reused --> Ended: idle again, summary built again from what is retained
    Ended --> Hidden: the person forgets or corrects the summary
    Hidden --> Hidden: later captures stay unsummarised, an explicit end is refused
    Ended --> Ended: a source observation is forgotten, the next sweep rebuilds the summary
    note right of Ended
        Summary memory is durable and ranked in recall.
        A rebuilt summary keeps its created time and
        verified commit, and its sensitivity can only rise.
    end note
```

A session with no retained observations gets no summary: an explicit end returns an error, the file store's sweep skips it, and PostgreSQL logs the failure and tries again after another idle period.

Where in the code: `crates/core/src/lib.rs` (`finish_idle_sessions`, `session_needs_summary`, `finish_session_staged`, `carry_summary_state`), `crates/storage/src/lib.rs` (`finish_idle_session`, `finish_session_summary`, `pending_since`).

### 2.4 A queued event's life

An event enters the outbox only when the one POST failed in a way that may pass (no connection, timeout, `408`, `429`, `5xx`). The hook's own pass after a good capture is "bounded"; `cairn_outbox.py replay` is the explicit pass. Other commands are `enqueue`, `status`, and `purge`.

```mermaid
stateDiagram-v2
    [*] --> Queued: delivery failed with a retryable error, max 256 events
    Queued --> Delivered: server accepts the event, it is removed from the file
    Queued --> Queued: another failure, attempts plus 1 and lastError kept
    Queued --> Skipped: 3 failed attempts, the hook pass ignores it
    Skipped --> Delivered: explicit replay still retries it and the server accepts
    Queued --> Dropped: server answers 409, removed, rejectedCount plus 1, lastRejected set
    Queued --> Purged: explicit replay gets 401, the whole queue is purged
    Queued --> Purged: cairn_outbox.py purge
    Delivered --> [*]
    Dropped --> [*]
    Purged --> [*]
    note right of Queued
        In the hook pass a 401 only stops the pass
        and adds an attempt. It never purges.
        An outage stops the pass at the first failure.
    end note
```

The queue file is bound to one server origin and one device token. A different origin, a rotated token, or a version mismatch makes the hook fail before it sends anything and leave the file untouched; there is no rebind command. `status` prints `pending`, `attempts`, `hasErrors`, `rejectedCount`, and `lastRejected`.

Where in the code: `scripts/cairn_outbox.py` (`_replay_loaded`, `replay_bounded`, `MAX_EVENTS`, `MAX_BOUNDED_ATTEMPTS`, `status`, `purge`), `scripts/cairn_hook.py` (`REPLAY_LIMIT`, `REPLAY_BUDGET`, `_enqueue`). Rules: [hook](./contracts/hook-v1.md#opportunistic-replay), [sync](./contracts/sync-layer3.md).

### 2.5 What is not captured

- Hook events that `examples/claude-code-hooks.json` does not register: `SessionStart` (the examples README says it can be added; it would record only the start input), and any other Claude Code event, such as tool calls before they run, notifications, or subagent and compaction events.
- Tools outside the `PostToolUse` matcher. Only `Bash`, `Edit`, `Write`, and `MultiEdit` are captured; reads, searches, web fetches, MCP tools, and the rest are not, unless you widen the matcher.
- Tool calls that fail may not fire `PostToolUse` at all (Claude Code reference, not checked here).
- Anything while the server is down and `CAIRN_OUTBOX` is unset, a full outbox (256), or a request refused with `401`, `409`, or another `4xx`.
- Project and branch outside a git checkout. The hook derives repository and branch from `cwd` with `git`, so captures from inside a checkout register the project and branch (checked against the file store). When `cwd` is not a git repository, or `git` is not installed, `context` stays empty and nothing is registered. A hook push that creates the session row stores the pushed context there (and fills an empty row, never replacing a real context), so the summary gets the repository and branch scope; a recall filtered by that repository and branch finds it.
- The agent's reasoning and any text that is not a prompt, a final reply, or one of the four tool inputs and outputs.

### 2.6 Setup

The full steps and a one-event check are in [`examples/README.md`](../examples/README.md); the order is:

1. Sign in to the console and open Devices.
2. Create a device for this machine. Copy the token; it is shown once. (Rotate it later from the same page; the old token stops working at once.)
3. Choose an outbox path outside any repository, such as `~/.cairn/outbox.json`.
4. Put `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`, and `CAIRN_OUTBOX` in the `env` block of the Claude Code settings file, or export them in the shell that starts `claude`. The hook reads no config file.
5. Copy the `hooks` block of `examples/claude-code-hooks.json` into `~/.claude/settings.json` (or one project's `.claude/settings.local.json`) and replace the path to this checkout.
6. Run the hook once by hand with a sample event, restart Claude Code, send a prompt, and look for it in `cairn search` or the console.

## 3. Capture path

This is what the server answers to one `sync/push`. The client side (hook, redaction, outbox) is in section 2. The server remembers each event id per person, so a repeat is harmless.

```mermaid
sequenceDiagram
    participant Hook as cairn_hook.py
    participant Srv as cairn-server
    participant Db as Store

    Hook->>Srv: POST sync/push with device token
    Srv->>Srv: check device token, validate, redact, set sensitivity
    alt new event
        Srv->>Db: store observation and sync event together
        Srv->>Db: create the session row if new, register project and branch
        Srv->>Db: close this person's idle sessions
        Srv-->>Hook: accepted, cursor
    else same eventId, same content
        Srv-->>Hook: accepted, duplicate true, original cursor
    else same eventId, different content
        Srv-->>Hook: 409 conflict, nothing written
    else token revoked or other 4xx
        Srv-->>Hook: 401 or 4xx
    end
```

Housekeeping after the write (project registration, idle close) never turns an accepted push into an error. A duplicate push does neither. Both stores now do the same on a new push; the differences that remain:

| Behavior | File store | PostgreSQL |
| --- | --- | --- |
| Project and branch registered from a capture | Yes, on a device push, `cairn capture`, and `memory_observe` | Yes, on every device push |
| Session row created by a hook push | Yes, with the pushed context; an empty row takes the first real one | Yes, with the pushed context; an empty row takes the first real one |
| Idle sessions closed on a push or session start | Yes, per person, at most 16 per run | Yes, per person, at most 16 per run |
| Idle sessions closed by the 5-minute timer | Yes, every person, under the store lock | Yes, every person, oldest pending first, at most 16 per run |
| Pull page size | Everything | 512 events and 512 deletions per call |
| Search ranking | Count of query-term occurrences | Full-text rank (`ts_rank_cd`) |
| Search result cap | None | 512 matches |
| `observe` with a web session | Accepted | 401 (device token only) |

Where in the code: `crates/core/src/lib.rs` (`sync_push`, `capture_for_account`), `crates/storage/src/lib.rs` (`sync_push`, `auto_register_project`). Rules: [hook](./contracts/hook-v1.md), [sync](./contracts/sync-layer3.md).

## 4. Recall path

An agent asks a question through `cairn context`, `cairn search`, or the MCP tools. The server looks only at that person's memories, drops anything forgotten, expired, or replaced by a live correction, and optionally narrows by session, worktree, branch, or repository. Ranking is plain text matching, not semantic search: the file store counts how often the query words appear, and PostgreSQL uses its full-text rank. The answer says how many matches were left out.

```mermaid
sequenceDiagram
    participant Agent as Agent
    participant Cli as cairn CLI or cairn mcp
    participant Srv as cairn-server
    participant Db as Store

    Agent->>Cli: query, limit, optional repository, branch, worktree
    Cli->>Srv: POST context, search, recall or why-changed with device token
    Srv->>Srv: device token resolves to one account
    Srv->>Db: load that person's memories only
    Db-->>Srv: candidate memories
    Srv->>Srv: drop forgotten, expired and corrected memories
    Srv->>Srv: if a filter was sent keep only matching scope
    Srv->>Srv: score each match, file store by word count, PostgreSQL by full-text rank
    Srv->>Srv: on equal scores decisions go first, then a newer contradicting memory, then memory id
    Note over Srv: why-changed instead puts memories with a stated reason first, then more code links
    Srv->>Srv: keep top limit, count the rest
    Srv-->>Cli: results, omitted_count, omitted_memory_ids, recovery hint
    Cli-->>Agent: JSON
```

Equal scores are the only place the order is adjusted: a project decision comes before any other memory, then, between two equal memories that list each other as contradictions, the newer one first, and otherwise memory id order. A higher score always wins. Nothing is hidden or resolved by this. Semantic (embedding) search is not implemented.

Scope filter in one line: `worktree` matches repository, branch, and worktree; `branch` matches repository and branch; `project` matches repository; `global` always matches; `session` matches the same session id. With no filter, every scope is returned. A missing field never widens a match. `recall` is the same route as `context`, and `smart-search` is the same route as `search`.

Where in the code: `crates/core/src/lib.rs` (`ranked_matches`, `memory_matches_filter`, `prefer_decision`, `prefer_newer_contradiction`, `build_context_pack`, `why_changed_context`), `crates/storage/src/lib.rs` (`search_window`, `why_changed_context`), `crates/client/src/tools.rs`. Rules: [continuity](./contracts/continuity-v1.md).

## 5. Project flow

Projects appear on their own when a capture carries a repository: any new device push or `cairn capture`, on both stores. (Claude Code sends no repository, so the hook derives it from `cwd` inside a git checkout; see sections 2.1 and 2.5.) Promotion is the only way personal memory becomes project memory, and it makes a copy. The personal memory is never changed or deleted by anything done to the project copy. The owner can do all of the following from the project page in the console, under "Project controls"; the CLI and MCP cover only promote.

```mermaid
flowchart TD
    A["Capture with context.repository and branch"] -->|"new push or capture,<br/>both stores"| B["Project and branch registered<br/>owned by the capturing person"]
    B --> C["Personal memory in branch or worktree scope"]
    C -->|"console Promote form, cairn promote<br/>or memory_promote<br/>not secret, not forgotten, same repository"| D["New decision memory<br/>scope project, kept in the project<br/>promoted_from points to the personal one"]
    C --> C2["Personal memory unchanged"]
    D -->|"console Verify form<br/>(REST projects/verify)"| E["verified_commit set, freshness 100<br/>stale flag when a newer commit is asked about"]
    C -->|"console Merge form or REST branches/id/merge<br/>owner picks a memory on that branch,<br/>a commit and a reason"| F{"Is the caller a device token and<br/>does the project have a member<br/>besides the owner?"}
    F -->|"no: console session, or solo project"| G["Merge allowed"]
    F -->|"yes"| H["Device token gets 403<br/>the console session is the way"]
    G --> I["Memory becomes a project decision,<br/>verified at the merge commit,<br/>branch marked merged, worktrees removed"]
    B -->|"console Close form or<br/>REST branches/id/close, reason required"| J["Branch closed without promotion<br/>worktrees removed, cannot be merged later"]
    D -->|"forget or delete the copy"| K["Only the copy and its shares hide<br/>personal memory stays"]
```

Every merge, close, verify, and promote form asks for confirmation (merge and close show what cannot be undone) and needs a reason or commit where the diagram says so. These actions are owner-only: members see shared decisions but no controls. A merged branch cannot be closed and a closed branch cannot be merged. The `projects/{id}/confirm` route, entity registration, and worktree registration have no console control, CLI command, or MCP tool.

Where in the code: `crates/core/src/lib.rs` (`auto_register_project`, `promote_memory`, `verify_project_memory`, `confirm_branch_merge`, `close_branch`), `crates/storage/src/lib.rs` (same names), `crates/server/src/lib.rs` (routes), `apps/web/components/project-owner-actions.tsx` (console forms). Rules: [project](./contracts/project-layer4.md).

## 6. Sharing flow

Sharing is by invitation. The owner invites an existing account (created by the admin, section 7), then shares one decision at a time, or turns on a policy that shares non-sensitive decisions automatically when they are promoted. The project page shows the current policy, Manual or Automatic, and offers a button to switch to the other. A member reads only what was shared with them.

```mermaid
sequenceDiagram
    participant Own as Owner
    participant Srv as cairn-server
    participant Mem as Member

    Own->>Srv: invite member email (account must already exist)
    Srv-->>Own: membership recorded
    alt manual policy, the default
        Own->>Srv: share one decision with the member
        Srv-->>Own: share recorded
    else auto policy, set per project
        Own->>Srv: promote a memory
        Note over Srv: not secret and not sensitive, so shared to every current member
    end
    Mem->>Srv: GET projects/id/shared
    Srv-->>Mem: active shared decisions only
    Own->>Srv: revoke share for one member
    Mem->>Srv: GET projects/id/shared
    Srv-->>Mem: that decision is gone
    Own->>Srv: remove member (web session only)
    Note over Srv: membership deleted, all active shares revoked
    Own->>Srv: GET revocations and conflicts
    Srv-->>Own: revoked shares with member email, conflicting decisions
    Mem->>Srv: GET revocations and conflicts
    Srv-->>Mem: own revoked shares, conflicts among shared decisions
```

Not shown because it does not exist: a share never copies the memory into the member's own memories, and it does not reach the member's devices through sync. The member's search and context do not return the owner's unshared memories. Turning the policy back to manual does not revoke shares already made. Decisions that a live correction replaced, forgotten, expired, and revoked ones are all hidden from the member.

The console lets an owner invite, share, revoke, delete, create a team, add or remove a member, and switch the share policy (with a confirmation that says what changes). Dismissing a share candidate has no console control, CLI command, or MCP tool; it is REST only. The policy has no CLI command or MCP tool either.

Where in the code: `crates/core/src/lib.rs` (`invite_project_member`, `share_project_memory`, `shared_project_memories`, `revoke_project_share`, `remove_project_team_member`, `auto_share_new_project_memory`, `project_revocations`), `apps/web/components/project-detail.tsx`, `apps/web/components/project-owner-actions.tsx`. Rules: [share](./contracts/share-layer5.md).

## 7. Sign-in, tokens, and permissions

The first visitor to an empty server creates the first account, and that account is the only admin. There is no open sign-up: every later account is created by the admin on the console's Accounts page. The console then creates device tokens, shows each one once, and stores only a hash. An agent gets its token through `cairn setup`, which reads it from standard input so it never lands in shell history. A device token can do almost everything an agent needs, but it is refused with `403` for account and device management, member removal, and merging on a shared project.

```mermaid
flowchart TD
    A["Open console on an empty server"] -->|"health says bootstrap_required"| B["Create first account<br/>email and password of 12 bytes or more<br/>this account is the admin"]
    B --> C["Web session, stored in httpOnly cookie<br/>valid 30 days"]
    A2["Later visits"] --> A3["Sign in with email and password"]
    A3 --> C
    C --> AD{"is_admin from auth/me?"}
    AD -->|"yes"| AC["Accounts page: create an account<br/>for a teammate, email and password"]
    AD -->|"no"| AN["No Accounts link<br/>the page says only an admin can create accounts"]
    AC --> NA["New account signs in on its own<br/>the admin's session does not change"]
    C --> D["Devices page: create a device by name"]
    D --> E["Device token shown once<br/>server keeps only a hash"]
    E --> F["cairn setup server-url<br/>token read from stdin"]
    F --> G["config.json mode 0600<br/>or CAIRN_URL and CAIRN_DEVICE_TOKEN"]
    G --> H["Agent calls REST with the device token"]
    C --> R["Devices page: rotate or revoke a device"]
    R --> S["The old token gets 401 on every agent route<br/>rotate shows a new token once"]
    H -.->|"device management, account creation<br/>or member removal"| X["403 web console session required"]
```

Hooks do not read `config.json`; they need `CAIRN_DEVICE_TOKEN` (and optionally `CAIRN_URL`) in their environment. Sign-in sessions last 30 days, on the server and in the console's cookie. Bootstrap, register, and login are limited to 10 tries per minute per email.

`GET auth/me` returns the signed-in account with `is_admin` for a web session or a device token; the console uses it to show or hide the Accounts link. There is exactly one admin and no way to change that role through the API. Creating an account (`POST auth/register`) answers:

| Caller | Answer |
| --- | --- |
| No bearer, or an unknown, expired, or revoked one | `401` |
| A device token, even the admin's own | `403`, web console session required |
| A web session of a non-admin account | `403`, only the admin account can do this |
| The admin's web session | `200`, a new account; the console drops the new account's session token and keeps the admin's |

The admin check runs before the per-email rate limit and before the email and password are validated. A duplicate email, or an email or password that fails validation, answers `400`.

| Action | Web session | Device token |
| --- | --- | --- |
| Create the first account, sign in | Yes, no bearer needed | `403` (bootstrap and login refuse a device bearer) |
| Create another account | Admin only; any other account `403` | `403` |
| Sign out, read audit history | Yes | `403` |
| Create, list, rotate, revoke devices | Yes | `403` |
| Remove a team member | Yes | `403` |
| Merge a branch on a project that has other members | Yes (owner) | `403` |
| Merge a branch on a solo project | Yes (owner) | Yes |
| Who am I (`auth/me`) | Yes | Yes |
| Start or end a session | Yes | Yes |
| Capture through `sync/push`, pull, sync diagnose | `401` | Yes |
| Recall, search, resume, why-changed, diagnose | Yes | Yes |
| Remember, correct, forget, delete, export, import | Yes | Yes |
| Projects, promote, verify, branches, invite, share, revoke, policy, teams | Yes | Yes |

Where in the code: `crates/core/src/lib.rs` (`bootstrap_account`, `register_account_as_admin`, `authenticate_admin`, `authenticate_session`, `authenticate_account`, `WEB_ONLY_ERROR`, `ADMIN_ONLY_ERROR`, `AUTH_SESSION_SECONDS`, `confirm_branch_merge`), `crates/storage/src/lib.rs`, `crates/server/src/lib.rs` (`auth_register`, `auth_me`, `AUTH_ATTEMPT_LIMIT`), `migrations/0013_admin_role.sql`, `apps/web/app/api/cairn/[...path]/route.ts` (cookie), `apps/web/components/login-form.tsx`, `accounts.tsx`, `devices.tsx`, `apps/web/hooks/use-account.ts`, `crates/client/src/lib.rs`. Rules: [auth](./contracts/auth-layer3.md), [ADR 0008](./adr/0008-self-hosted-account-authentication.md).

## 8. Sync between devices

Every accepted capture gets a cursor number in that person's stream. A device asks for everything after the cursor it last saw and gets new events plus deletion instructions. The server hands out cursors one person at a time, in order, so a pull never skips an item that is still being written. Two devices do not merge or resolve content: a conflict is only the same event id arriving with different content, and that is refused with `409`.

```mermaid
sequenceDiagram
    participant A as Device A
    participant Srv as cairn-server
    participant B as Device B

    A->>Srv: push event
    Note over Srv: cursor 41, one person at a time under a lock
    Srv-->>A: accepted, cursor 41
    B->>Srv: pull after cursor 40
    Srv-->>B: event 41, deletions, new cursor
    B->>Srv: pull after cursor 41
    Srv-->>B: nothing new
    A->>Srv: forget an item
    Note over Srv: a deletion gets its own cursor
    B->>Srv: pull
    Srv-->>B: deletion instruction for that item
    A->>Srv: push same eventId, different content
    Srv-->>A: 409 conflict, nothing written
```

Pull and sync diagnose are REST routes that only the smoke scripts call. No shipped client reads the sync stream: the hook only pushes, and `cairn` and `cairn mcp` ask the server directly (search, context, resume) every time. A deletion instruction is therefore never delivered to anything today, and that is fine, because no client keeps a local copy of memory that could go stale. The only local data on a device is the outbox, which holds events that were never delivered, so the server has nothing to forget for them yet. A forget or delete takes effect at the server, and the next recall already leaves the item out. The pull route and its deletion instructions are the hook for a future client that keeps a local cache; such a client would have to pull to learn about deletions. Sync diagnose always reports `pending_events` and `rejected_events` as `0`; the outbox's own `status` reports its queue and rejected count.

Where in the code: `crates/core/src/lib.rs` (`sync_push`, `sync_pull`), `crates/storage/src/lib.rs` (`sync_pull`, `lock_sync_cursor`, `MAX_PULL_EVENTS`), `scripts/smoke_layer3.sh`. Rules: [sync](./contracts/sync-layer3.md).

## 9. Use cases

Mermaid has no use-case diagram type, so each case is a flowchart: a person on the left, the steps they or their agent take on the right.

### 9a. A solo developer's day with an AI agent

Nothing in the day needs a manual save. The one deliberate act is asking for context at the start.

```mermaid
flowchart LR
    Dev(["Developer"])
    Ag(["AI agent"])
    Dev --> W1["Start work in a repository"]
    W1 --> R1["Agent runs cairn resume or cairn unresolved<br/>or the MCP memory_resume"]
    R1 --> R2["Yesterday's summary and open items"]
    W1 --> W2["Agent works, hook sends each event"]
    Ag --> W2
    W2 --> S1["Observations stored, secrets redacted"]
    S1 --> S2["Project and branch registered when the<br/>capture carries a repository name"]
    W2 --> Q1["Agent asks cairn context for a past decision"]
    Q1 --> Q2["Ranked matches and omitted count"]
    W2 --> E1["Later: 30 idle minutes, then the next push,<br/>session start, or the 5-minute server timer"]
    E1 --> E2["Session summary memory made<br/>ready for tomorrow"]
```

### 9b. A team owner shares decisions with a member

```mermaid
flowchart LR
    Own(["Project owner"])
    Mem(["Team member"])
    Own --> P1["Agent promotes a memory (cairn promote)<br/>or the owner uses the console Promote form"]
    P1 --> P2["Project decision with provenance"]
    Own --> I1["Invite member email<br/>console or cairn project-invite"]
    I1 --> I2["Member account must already exist"]
    P2 --> S1["Share decision<br/>console or cairn project-share"]
    I2 --> S1
    S1 --> M1["Member signs in to the console"]
    Mem --> M1
    M1 --> M2["Project page shows shared decisions"]
    Own --> V1["Revoke a share or remove the member<br/>console"]
    V1 --> V2["Member no longer sees the decision<br/>revocation listed in the report"]
```

### 9c. Setting up a new device or agent

```mermaid
flowchart LR
    Own(["Owner"])
    Own --> A1["Sign in to the console<br/>(an admin first creates the account<br/>on the Accounts page)"]
    A1 --> A2["Devices page: create a device"]
    A2 --> A3["Copy the token, shown once"]
    A3 --> B1["On the device: cairn setup server-url<br/>paste token on stdin"]
    A3 --> B2["For the hook: set CAIRN_DEVICE_TOKEN<br/>and CAIRN_URL in its environment"]
    B1 --> C1["Check: cairn project-list"]
    B1 --> C2["Add cairn mcp to the agent"]
    B2 --> C3["Optional: set CAIRN_OUTBOX<br/>for outage queueing"]
    B2 --> C4["Copy the hooks block of<br/>examples/claude-code-hooks.json<br/>into Claude Code settings"]
    C1 --> D1["Devices page shows last seen"]
    C4 --> D1
```

### 9d. Correcting or forgetting a wrong memory

Pick the action by what is wrong. A correction keeps history; forget hides and keeps the record; a project delete only touches the project copy.

```mermaid
flowchart TD
    Q{"What is wrong?"}
    Q -->|"A fact is outdated<br/>or partly wrong"| C["cairn correct id new-text"]
    C --> C2["New correction memory<br/>old one stays linked and hidden from recall"]
    Q -->|"Should never be recalled<br/>or contains private data"| F["cairn forget id"]
    F --> F2["Tombstone: hidden from recall,<br/>record kept in storage and export"]
    F2 --> F3["Every later recall already leaves it out<br/>no client keeps a local copy"]
    Q -->|"A shared project decision"| D["Console: Delete memory<br/>or Revoke share"]
    D --> D2["Only the project copy hides<br/>personal memory is untouched"]
```

### 9e. The reports the owner looks at in the console

```mermaid
flowchart TD
    L["Sign in"] --> O["Overview"]
    O --> O1["Memory by state:<br/>temporary, durable, sensitive, expired"]
    O --> O2["Decisions by project"]
    O --> O3["Recent decisions"]
    L --> PL["Projects list"]
    PL --> PD["Project page"]
    PD --> R1["Decisions<br/>with share, revoke, delete"]
    PD --> R2["Team and owner actions:<br/>share policy, promote, verify,<br/>merge and close a branch"]
    PD --> R3["Conflicts: decisions that contradict"]
    PD --> R4["Freshness: verification history"]
    PD --> R5["Share candidates per member"]
    PD --> R6["Revoked shares"]
    L --> DV["Devices"]
    DV --> DV1["Create, list, rotate, revoke devices"]
    L -->|"admin only"| AC["Accounts"]
    AC --> AC1["Create an account for a teammate"]
```

### 9f. The admin adds a teammate

The teammate's account must exist before the owner can invite them to a project (section 6). Only the admin can create it.

```mermaid
flowchart LR
    Ad(["Admin<br/>first account"])
    Ad --> N1["Sign in to the console"]
    N1 --> N2["Accounts page:<br/>email, password of 12 bytes or more, confirm"]
    N2 --> N3["Account created<br/>admin stays signed in"]
    N3 --> N4["Teammate signs in with that email<br/>and creates their own devices"]
    N3 --> N5["A project owner can now invite<br/>that email to a project"]
```

Where in the code: `apps/web/components/overview.tsx`, `project-detail.tsx`, `project-owner-actions.tsx`, `project-reports.tsx`, `devices.tsx`, `accounts.tsx`, `crates/client/src/bin/cairn.rs`.

## 10. Tools at a glance

| Tool | What it is for | Who uses it | Credential |
| --- | --- | --- | --- |
| `scripts/cairn_hook.py` | Turns one agent event (Claude Code hook input, section 2.1) into a `sync/push`; optional outbox on outage | The agent host, automatically | `CAIRN_DEVICE_TOKEN` (env), `CAIRN_URL` (env) |
| `scripts/cairn_outbox.py` | Manual queue control: `enqueue`, `replay`, `status`, `purge` | An operator or script | Device token via `--token` (replay only) |
| `cairn` CLI | One REST call per command. `setup`, `mcp`, `start`, `capture`, `end`, `search`, `context`, `why-changed`, `resume`, `unresolved`, `forget`, `lesson`, `correct`, `export`, `import`, `project-register`, `project-list`, `promote`, `project-memories`, `project-freshness`, `project-invite`, `project-share`, `project-shared`, `project-revoke`, `project-team-create`, `project-team`, `project-team-add`, `delete-memory` | Agents, and people setting up | Device token from `cairn setup` or env |
| `cairn mcp` | Same tool table over MCP stdio. 32 tools; the 26 that match a CLI command are in the CLI row, and six are MCP only: `memory_save`, `memory_recall`, `memory_consolidate`, `memory_smart_search`, `memory_diagnose`, `memory_reflect` | AI agents | Device token |
| Web console | Pages: sign-in (`/` and `/login`), `/dashboard`, `/projects`, `/projects/[id]` (owner controls: share policy, promote, verify, merge, close), `/devices` (create, rotate, revoke), `/accounts` (admin only, linked from the sidebar only when `auth/me` says `is_admin`) | The owner, invited members, and the admin | Web session cookie |
| REST server | `cairn-server`, routes under `/cairn-memory/*`, plus `/viewer` | Everything above | Per route: web session, device token, or both |
| Docker dev stack | PostgreSQL 16, server, console on 127.0.0.1 ports 55432, 58080, 53000; `scripts/dev_up.sh` and `scripts/dev_down.sh` | Developers of Cairn | A generated database password in `.env.dev` |

Actions with no CLI command, MCP tool, or console control today (REST only): `projects/{id}/confirm`, dismissing a share candidate, entity registration, and worktree registration. Branch merge and close, verify, share policy, and device rotate have console forms but still no CLI command or MCP tool; account creation is console only by design. Semantic (embedding) search is not implemented; the health route reports `semantic_search: disabled`.

Where in the code: `crates/client/src/tools.rs` (tool table), `crates/client/src/bin/cairn.rs` (commands), `crates/server/src/lib.rs` (routes), `apps/web/components/app-sidebar.tsx` (pages), `examples/claude-code-hooks.json`, `docker-compose.dev.yml`.
