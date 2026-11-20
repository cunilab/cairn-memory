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
  history, and writes the selected config file with mode `0600`, through a fresh temporary
  file and a rename; a config directory it creates gets mode `0700`. The selected file is
  `CAIRN_CONFIG` when that variable is non-empty after trimming, otherwise
  `$XDG_CONFIG_HOME/cairn/config.json` (else `~/.config/cairn/config.json`, else `%APPDATA%`).
  That file's `url` and `token` are the only server URL and device credential for the CLI
  and stdio MCP. `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are not configuration. If either
  name is present, including an empty or whitespace-only value, it is ignored and one
  stderr line names the variable without its value. Missing
  configuration is an error that names the selected file and `cairn setup`.
- Server URL: `https`, or `http` only for a loopback host (`localhost`, `127.0.0.0/8`,
  `::1`). Plain `http` to any other host is refused at setup and at load. A URL may not
  carry credentials, a query, or a fragment. It may carry a path prefix (a trailing slash is
  dropped), for a server behind a reverse proxy mount such as `https://cairn.example.test/agent`;
  every request goes to `<url>/cairn-memory/...`.
- The client never follows a redirect, so the token cannot leave the configured origin, and
  never prints the token. Errors show the server's own message, for example
  `HTTP 403: web console session required; device tokens cannot do this`. A genuine HTTP 401
  names the selected config file and says to run `cairn setup <server-url>` with
  `CAIRN_CONFIG` set to that file. The credential is removed from the response text
  before the 300-character clip. The 401 stays inside
  the CLI's stderr line or the MCP tool result; it does not add a second stdout record.
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
| `memory_save` | `POST /remember` | Save a typed memory (`kind`, `title`, `summary`, `reason`, per-kind `fields`, `evidence`, `supersedes`, `contradicts`, optional `scope`) |
| `memory_list`, `memory_get` | `GET /memories`, `GET /memories/{id}` | List readable records or get every field |
| `memory_recall` | `POST /recall` | Return a bounded context pack |
| `memory_consolidate` | `POST /session/end` | End a session; the server distills a handoff note |
| `memory_smart_search` | `POST /smart-search` | Search durable memories |
| `memory_why_changed` | `POST /why-changed` | Return decisions, reasons, evidence, relationships, and conflict signals |
| `memory_diagnose` | `GET /diagnose` | Report counts by kind, status and scope, and job counts |
| `memory_lesson_save` | `POST /remember` | Same as `memory_save`; `kind` defaults to `gotcha` |
| `memory_feedback` | `POST /feedback` | Vote a memory `helpful` or `wrong` |
| `memory_reflect` | `POST /context` | Return hybrid retrieval candidates; local embeddings when enabled |
| `memory_session_start` | `POST /session/start` | Start or resume a session |
| `memory_observe` | `POST /observe` | Capture one bounded, idempotent observation |
| `memory_session_end` | `POST /session/end` | Alias for session consolidation |
| `memory_search` | `POST /search` | Hybrid search |
| `memory_context` | `POST /context` | Bounded context assembly |
| `memory_resume` | `POST /resume` | Recover the caller's recent handoffs for current work context |
| `memory_unresolved` | `POST /unresolved` | Recover handoffs with open questions and open contradictions |
| `memory_forget` | `POST /forget` | Forget a memory: its text is erased and it cannot be saved again |
| `memory_export` | `POST /export` | Return the caller's visible memories and links |
| `memory_contradict` | `POST /contradict` | Declare that one visible memory contradicts another |
| `memory_supersede` | `POST /supersede` | Set `supersedes` on an existing memory |
| `memory_link` | `POST /links` | Add one typed link; edit rights on the source |
| `memory_unlink` | `POST /links/remove` | Remove one typed link |
| `memory_graph` | `GET /graph` | Visible memory link graph, with optional scope, project, relation, and cap |

Project tools run the same routes as the web console, with the same
account scope. Their behavior is owned by the contract named here:

| Tool | Owner |
| --- | --- |
| `memory_project_list`, `memory_project_register`, `memory_project_memories` | [CM-CONTRACT-009](project-layer4.md) |
| `memory_project_members`, `memory_delete` (an alias of forget) | [CM-CONTRACT-009](project-layer4.md) |

Removed in 0.1.6 with their routes: `memory_import`, `memory_correct` (a correction is a save with `supersedes`), `memory_similarity_reviews`, `memory_review_dismiss`, `memory_review_contradict`, `memory_review_supersede`, `memory_project_verify` and `memory_project_freshness`. Removed earlier with the local-store mode: `memory_sessions` (it listed the file store's unscoped
sessions), `memory_account_bootstrap` and `memory_account_register` (account creation is a
web action), and `memory_project_team_remove` (member removal is web-only; a device token is
`403`, so the tool would only invite retries). Since 2026-11-16 there is no invite, team
member add or member removal at all, so `memory_project_invite` and
`memory_project_team_add` are gone too; a teammate joins a project by capturing in a checkout
whose `.cairn/project.json` names it ([project-layer4](project-layer4.md)). `cairn init`, which
writes that file, is a CLI command only. `memory_project_list` is
new: an agent needs a project id before saving project memory. Argument names are the
camelCase names of the REST bodies; path ids (`projectId`, `memoryId`) are percent-encoded
into the route.

Tool results include a text content block and `structuredContent`. A failed call, including a
server `4xx`, is returned as a tool result with `isError: true` and the server's message;
malformed JSON-RPC requests and unknown tools or protocol methods use JSON-RPC errors. A tool
checks its required arguments before the request.

## Agent-facing results (v0.1.5)

`structuredContent` is always a JSON object. A tool whose REST body is an array
is wrapped, including when the array is empty:

| Tool | Object key |
| --- | --- |
| `memory_list`, `memory_project_memories` | `memories` |
| `memory_project_list` | `projects` |
| `memory_project_members` | `members` |
| `memory_search`, `memory_smart_search` | `results` |

`initialize.serverInfo` is `name: cairn-memory`, `title: Cairn`. The shared
instruction text says these are Cairn tools. Protocol names stay as in the
table above, including `memory_save` and `memory_get`. OpenCode names a tool
`<server>_<tool>` ([Names](https://opencode.ai/docs/mcp-servers/#names)); Cairn's
server key is `cairn`, so the host calls `cairn_memory_save`. Renaming the
protocol tools is not required. The MCP schema requires `structuredContent` to
be an object ([SEP-2106](https://modelcontextprotocol.io/seps/2106-json-schema-2020-12)).

`memory_save` and `memory_lesson_save` return this receipt on both channels. The text is compact JSON of the
same object. Errors stay errors.

```json
{"id":"mem-1","status":"active","deduplicated":false}
```

An identical explicit save from the same owner returns the existing `id` with `deduplicated: true`; newly
supplied evidence is merged. A save matching a forgotten memory is an error (`forgotten`). See
[explicit-save identity](continuity-v1.md#explicit-save-identity-v016).

`memory_consolidate` and `memory_session_end` return `{"status":"queued"}` when the server will distill a
handoff, or `{"status":"empty"}` for a session with no retained observations, which is a normal close. See
[empty-session close](continuity-v1.md#empty-session-close-v016).

`memory_get` and `memory_export` return the server JSON unchanged as compact text. The CLI prints pretty
server JSON for every command.

`memory_search`, `memory_smart_search`, `memory_recall`, `memory_context` and `memory_reflect` default to short
cards on both channels: `id`, `kind`, `scope`, `title`, `summary`, `score`. The result keeps `query`, `results`
and `more` (the number of hits left out by the budget); reflect also keeps `mode` and `note`. On a card `title`
is at most 120 UTF-8 bytes and `summary` at most 240; a longer value is cut on a character boundary and ends
with `…`.

```json
{"query":"hook auth","results":[{"id":"mem-1","kind":"gotcha","scope":"project","title":"Hook auth","summary":"401 names the config file.","score":0.5}],"more":0}
```

`expanded: true` is the only wider option; it is removed before the REST call. Each hit then also carries
`project_id`, `status`, `reason`, `fields`, `evidence`, `entities`, `links` and `context`, with empty arrays and
objects omitted. `memory_why_changed`, `memory_resume` and `memory_unresolved` return their hits with empty values
omitted. Authorization is unchanged. Search and context apply the relevance floor in
[CM-CONTRACT-006](continuity-v1.md).

Other MCP objects use compact JSON and omit nulls and empty strings on both channels. They keep false, zero and
flags such as `accepted` and `duplicate`. Lists are wrapped: `memories`, `projects`, `members`, `results`.
`memory_graph` keeps JSON `structuredContent`; its text is tab-delimited TOON
([spec 4.4](https://github.com/toon-format/spec/blob/main/SPEC.md)), the only shape that encoder supports.

## Scoped records and CLI

The record shape, visibility and default scope rules are defined in [the scope contract](project-layer4.md#memory-scopes-and-record). MCP `memory_save` and `memory_lesson_save` take `kind`, `title`, `summary`, `reason`, a per-kind `fields` object, `evidence`, `supersedes`, `contradicts` and an optional `scope`. The lesson alias defaults `kind` to `gotcha`. Saves in an initialized checkout fill `context.project_id` from its committed file, which makes the default scope `project`.

CLI `cairn save '<JSON-record>'` and `cairn lesson '<JSON-record>'` accept the same record, or read one JSON object from stdin when no argument is supplied. `cairn memories` lists readable notes; `cairn memory <id>` prints a complete record. The CLI prints the server's fields. MCP shapes are in [Agent-facing results](#agent-facing-results-v015).

```sh
cairn save '{"kind":"preference","title":"Concise replies","summary":"Answer in one paragraph.","reason":"Reduce reading time.","fields":{"example":"one paragraph"}}'
cairn memories
cairn memory memory-id
```

Other commands retain `start`, `capture`, `end`, `search`, `context`, `why-changed`, `resume`, `unresolved`, `forget`, `contradict`, `supersede`, `export`, `project-list`, `project-members`, `project-memories`, `delete-memory` and `init`. Account/device management remains in the web console. `memory_project_memories` lists live project memories. Only the author can forget a memory.

All search/context routes use [hybrid recall](continuity-v1.md#hybrid-recall-v014); no external model API is used.

## Layer 2 context filters

The query tools `memory_recall`, `memory_context`, `memory_reflect`,
`memory_smart_search`, and `memory_search` accept optional `sessionId`,
`context`, and `includeCaptures` arguments. `includeCaptures` defaults to false.
`memory_why_changed` accepts it too. Resume and unresolved do not. Scope
behavior, capture eligibility, and the relevance floor are defined in
[CM-CONTRACT-006](continuity-v1.md). CLI `cairn search` and `cairn context`
take `--include-captures` for the same opt-in.

## Deliberate limitations

- The client needs a running server and a device token created in the web console; there is
  no offline or local-file mode, and the CLI and MCP no longer create accounts.
- `memory_observe` is a device-token capture; a web session is refused with `401`. It returns
  `{accepted, duplicate, observation, project_attachment}`.
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
  the server router on a `TestDb` schema (capture, recall, session, scoped save, a `403` on
  web-only routes, revocation); it needs `CAIRN_TEST_DATABASE_URL`.
- `scripts/smoke_mcp.sh` drives `cairn mcp` against a spawned `cairn-server`.

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
