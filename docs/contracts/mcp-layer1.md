---
id: CM-CONTRACT-002
title: "Layer 1 MCP Compatibility Contract"
status: draft
---

# Layer 1 MCP Compatibility Contract

This contract defines the Cairn MCP transport and the `cairn` CLI, which share one
tool table (`crates/client/src/tools.rs`). Both are network clients: each tool call is
one REST call to a Cairn server, authenticated with a device token. Neither opens a store
file. It is intentionally a small compatibility surface rather than a claim of parity with
the upstream full tool registry. The credential model is in
[CM-CONTRACT-008](auth-layer3.md); the decision is in
[ADR 0008](../adr/0008-self-hosted-account-authentication.md).

## Transport

- Command: `cairn mcp` (the `cairn-server mcp --store` command no longer exists)
- Configuration: `cairn setup <server-url>` reads the device token from stdin (typed without
  echo on a terminal, one line when piped) so it never appears in the process list or shell
  history, and writes `$XDG_CONFIG_HOME/cairn/config.json` (else
  `~/.config/cairn/config.json`, else `%APPDATA%`) with mode `0600`, through a fresh temporary
  file and a rename; a config directory it creates gets mode `0700`. `CAIRN_URL` and
  `CAIRN_DEVICE_TOKEN` together replace the file. Setting only one of them is an error, so a
  stored token is never sent to another server. Missing configuration is an error that names
  `cairn setup`.
- Server URL: `https`, or `http` only for a loopback host (`localhost`, `127.0.0.0/8`,
  `::1`). Plain `http` to any other host is refused at setup and at load. A URL may not
  carry credentials, a query, or a fragment. It may carry a path prefix (a trailing slash is
  dropped), for a server behind a reverse proxy mount such as `https://cairn.example.test/agent`;
  every request goes to `<url>/cairn-memory/...`.
- The client never follows a redirect, so the token cannot leave the configured origin, and
  never prints the token. Errors show the server's own message, for example
  `HTTP 403: web console session required; device tokens cannot do this`.
- TLS is rustls through the `ureq` crate (no OpenSSL, no async runtime in the client). It
  verifies certificates with the operating system's trust store, so a server behind a
  private CA works once that CA is trusted by the system.
- Plain `http` (loopback) never uses `HTTP_PROXY`, `HTTPS_PROXY`, or `ALL_PROXY`, so the
  token is not handed to a proxy. `https` URLs honour those variables.
- A tool argument that fills a path segment may not be `.` or `..`; the call fails before any
  request.
- Transport: newline-delimited JSON-RPC 2.0 over stdin/stdout. A line that is not UTF-8 or
  not JSON gets `-32700`; valid JSON that is not a single request object (a batch, a response,
  no `method`) or a line over 8 MiB gets `-32600`; the server keeps reading either way.
  A message with an `id` member, including `"id": null`, always gets a reply. A message
  without one is a notification: it gets no reply and runs nothing, so a `tools/call`
  notification causes no REST call.
- Diagnostics: stderr only; stdout is reserved for protocol messages
- Handshake: `initialize`, `notifications/initialized`
- Discovery: `tools/list`
- Invocation: `tools/call`
- Probe: `ping`
- Protocol version returned by this adapter: `2025-06-18`

## Tool manifest

Each tool maps to one route. No tool takes an account token: the device token in the
configuration is the only credential, and the server scopes every call to its account.

| Tool | Route | Behavior |
| --- | --- | --- |
| `memory_save` | `POST /remember` | Save a scoped structured record; scope, title, summary and reason required |
| `memory_list`, `memory_get` | `GET /memories`, `GET /memories/{id}` | List readable records or get every field |
| `memory_recall` | `POST /recall` | Return a bounded context pack |
| `memory_consolidate` | `POST /session/end` | End a session and create its evidence-backed memory |
| `memory_smart_search` | `POST /smart-search` | Search durable memories |
| `memory_why_changed` | `POST /why-changed` | Return decisions, reasons, evidence, relationships, and conflict signals |
| `memory_diagnose` | `GET /diagnose` | Report the account's lifecycle counts |
| `memory_lesson_save` | `POST /remember` | Save a scoped record; defaults to gotcha unless kind is explicit |
| `memory_reflect` | `POST /context` | Return hybrid retrieval candidates; local embeddings when enabled |
| `memory_session_start` | `POST /session/start` | Start or resume a session |
| `memory_observe` | `POST /observe` | Capture one bounded, idempotent observation |
| `memory_session_end` | `POST /session/end` | Alias for session consolidation |
| `memory_search` | `POST /search` | Hybrid search |
| `memory_context` | `POST /context` | Bounded context assembly |
| `memory_resume` | `POST /resume` | Recover recent session summaries for current work context |
| `memory_unresolved` | `POST /unresolved` | Recover session summaries with unresolved work |
| `memory_forget` | `POST /forget` | Persist forget suppression for an event or memory |
| `memory_export` | `POST /export` | Return a versioned portable memory bundle |
| `memory_import` | `POST /import` | Merge a versioned bundle idempotently |
| `memory_correct` | `POST /correct` | Supersede a memory or observation with a corrected durable memory |

Project tools run the same routes as the web console, with the same
account scope. Their behavior is owned by the contract named here:

| Tool | Owner |
| --- | --- |
| `memory_project_list`, `memory_project_register`, `memory_project_memories`, `memory_project_freshness` | [CM-CONTRACT-009](project-layer4.md) |
| `memory_project_members`, `memory_project_verify`, `memory_delete` | [CM-CONTRACT-009](project-layer4.md) |

Removed with the local-store mode: `memory_sessions` (it listed the file store's unscoped
sessions), `memory_account_bootstrap` and `memory_account_register` (account creation is a
web action), and `memory_project_team_remove` (member removal is web-only; a device token is
`403`, so the tool would only invite retries). Since 2026-11-16 there is no invite, team
member add or member removal at all, so `memory_project_invite` and
`memory_project_team_add` are gone too; a teammate joins a project by capturing in a checkout
whose `.cairn/project.json` names it ([project-layer4](project-layer4.md)). `cairn init`, which
writes that file, is a CLI command only. `memory_diagnose` returns the lifecycle
summary of `GET /diagnose`, not the earlier local storage counts. `memory_project_list` is
new: an agent needs a project id before saving project memory. Argument names are the
camelCase names of the REST bodies; path ids (`projectId`, `memoryId`) are percent-encoded
into the route.

Tool results include a text content block and `structuredContent`. A failed call, including a
server `4xx`, is returned as a tool result with `isError: true` and the server's message;
malformed JSON-RPC requests and unknown tools or protocol methods use JSON-RPC errors. A tool
checks its required arguments before the request.

Capture and explicit-save tools accept the nested Layer 1 `lifecycle` object
defined in [CM-CONTRACT-004](lifecycle-v1.md).

## Scoped records and CLI

The flat common and per-type fields, visibility and authority rules are defined in [the scope contract](project-layer4.md#memory-scopes-and-record). MCP `memory_save` and `memory_lesson_save` expose every field and require `scope`, `title`, `summary`, `reason`. The lesson alias defaults `kind` to `gotcha`; an explicit kind is preserved. Project saves in an initialized checkout fill `context.project_id` from its committed file when absent. Scope is always explicit.

CLI `cairn save '<JSON-record>'` and `cairn lesson '<JSON-record>'` accept the same fields, or read one JSON object from stdin when no argument is supplied. `cairn memories` lists readable notes; `cairn memory <id>` prints a complete record. CLI and MCP print the server's fields without stripping them.

```sh
cairn save '{"scope":"personal","kind":"preference","title":"Concise replies","summary":"Answer in one paragraph.","reason":"Reduce reading time.","preference":"concise","example":"one paragraph"}'
cairn memories
cairn memory memory-id
```

Other commands retain `start`, `capture`, `end`, `search`, `context`, `why-changed`, `resume`, `unresolved`, `forget`, `correct`, `export`, `import`, `project-list`, `project-members`, `project-memories`, `project-verify`, `project-freshness`, `delete-memory` and `init`. Account/device management remains in the web console. `memory_project_memories` includes raw captures and summaries. `memory_project_verify`, correct and delete retain project author/owner checks; global correct/delete allow author or admin. `memory_promote`, `promote`, `promote-preview` and private per-repository lists are removed.

All search/context routes use [hybrid recall](continuity-v1.md#hybrid-recall-v014); no external model API is used.

## Layer 2 context filters

The query tools `memory_recall`, `memory_context`, `memory_reflect`,
`memory_smart_search`, and `memory_search` accept optional `sessionId` and
`context` arguments. Their scope behavior is defined in
[CM-CONTRACT-006](continuity-v1.md).

## Deliberate limitations

- The client needs a running server and a device token created in the web console; there is
  no offline or local-file mode, and the CLI and MCP no longer create accounts.
- `memory_observe` on the PostgreSQL server is a device-token sync push (it records a sync
  event); on the file store it accepts either credential. Both return the same
  `{accepted, duplicate, observation}` shape.
- `memory_reflect` is retrieval-only until a provider-backed reflection
  lifecycle exists.
- The adapter supports the compatibility handshake used by the current Layer 1
  baseline; modern protocol-era discovery is a later transport task.
- The tool schemas are Cairn-owned compatibility schemas. Exact upstream
  argument and result parity remains a verification task.

## Evidence

- `crates/client/src/tools.rs` holds the tool table and route mapping;
  `crates/client/src/mcp.rs` holds the protocol and its unit tests;
  `crates/client/src/lib.rs` holds the configuration and HTTP client with their unit tests
  (file mode, environment override, loopback-only `http`, redirect refusal, token never in
  output or errors).
- `crates/client/tests/agent_flow.rs` runs the real `cairn` binary and `cairn mcp` against
  the file-store router (capture, recall, session, scoped save, a `403` on web-only
  routes, revocation) and, as an ignored test with `CAIRN_TEST_DATABASE_URL`, against the
  PostgreSQL router.
- `scripts/smoke_mcp.sh`, `scripts/smoke_layer1.sh`, `scripts/smoke_layer4_cli.sh`,
  `scripts/smoke_layer5_cli.sh`, and `scripts/smoke_export.sh` drive the binaries against a
  spawned `cairn-server`.

## Agent guidance (v0.1.4)

MCP `initialize.instructions` returns the same compact text used by hook and
instruction-file integration (`scripts/cairn_instructions.txt`, at most 2048
UTF-8 bytes). It tells the agent when to recall/save, choose project/personal/global
scope explicitly, supply title/summary/reason and fill known per-type fields and
evidence. Save/lesson tool descriptions repeat their required fields and scope
visibility; missing causal fields must never be invented. Tool schemas and REST
validation still enforce required fields. This is agent guidance, not a
server-side memory-writing LLM. Setup, removal, per-agent capabilities and limits
are owned by [the hook contract](hook-v1.md#agent-instructions-and-setup-v014).
