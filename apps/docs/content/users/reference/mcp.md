---
id: CM-REF-001
title: "MCP tools"
status: draft
description: "JSON-RPC transport, arguments and responses for Cairn’s memory and skill tools."
---

# MCP tools

Model Context Protocol JSON-RPC specification, tool argument schemas, and compact result formats.

The `cairn mcp` stdio server exposes five memory tools and four global skill tools. In Hermes, Cairn's memory provider supplies these same nine tools natively.

## Transport

- **Command**: `cairn mcp` running newline-delimited JSON-RPC 2.0 over stdin/stdout. Diagnostics output to stderr only.
- **Protocol**: Returns protocol version `2025-06-18`. Supported methods: `initialize`, `notifications/initialized`, `tools/list`, `tools/call`, `ping`.
- **Limits**: Maximum JSON line size is 8 MiB (exceeding returns error `-32600`). Non-JSON input returns `-32700`.
- **Authentication**: Authenticates using the local device token ([Authentication](../../technical/security.md)).

## Instructions

`initialize.instructions` provides up to 700 bytes of agent guidance (`cairn-memory`). Memory content is strictly treated as data, never executable instructions.

## Tools

`tools/list` returns schemas for all nine tools (snapshot in `crates/client/tests/fixtures/tools_list.json`):

| Tool | REST endpoint | Arguments | Description |
| --- | --- | --- | --- |
| `memory_search` | `POST /search` | `query` (str), `kind` (str), `limit` (int 1–20, default 5) | Hybrid search across personal, project, and global memories |
| `memory_get` | `POST /get` | `ids` (list 1–10), `detail` (bool, default false) | Retrieve full memory records, links, and detailed notes |
| `memory_save` | `POST /remember` | `kind`, `title` (≤80B), `summary` (≤240B), `reason`, `detail` (≤8KB), `fields`, `evidence`, `supersedes`, `contradicts`, `scope`, `aliases`, `items` | Save or update a memory record (validates quality) |
| `memory_feedback` | `POST /feedback` | `id` (str), `vote` (`helpful` \| `wrong`) | Vote on memory accuracy to adjust confidence rankings |
| `memory_forget` | `POST /forget` | `id` (str) | Hide an owned memory from normal recall |
| `skill_search` | `POST /skills/search` | `query` (str), `limit` (int 1–20), `offset` (int) | Search global shared skill library |
| `skill_get` | `POST /skills/get` | `name_or_id` (str), `version` (optional int) | Fetch Markdown body and metadata for a skill |
| `skill_save` | `POST /skills/save` | `name`, `title`, `description`, `body`, `change_note` | Publish or append an immutable revision to a skill |
| `skill_feedback` | `POST /skills/feedback` | `name_or_id`, `version`, `vote` (`helpful` \| `wrong`), `note` | Record per-version feedback on a skill |

### Quality validation & defaults

- **Reason requirement**: `reason` (≥ 20 chars) is mandatory for all kinds except `note`.
- **Corrections**: Save with `supersedes: "<old_id>"` to replace an owned or same-project memory.
- **Default scope**: `preference` and `note` default to `personal`. Other kinds default to `project` if the caller belongs to the active project context, otherwise `personal`.

## Results

Tool output returns compact JSON directly in `content[0].text`:

- **`memory_search`**:
  ```json
  {"results":[{"id":"mem_123","kind":"decision","title":"Use Postgres SKIP LOCKED","summary":"Background jobs run...","score":1.0,"trust":"proven · 4 helpful · used 12×"}],"more":3}
  ```
- **`memory_get`**:
  ```json
  {"memories":[{"id":"mem_123","kind":"decision","title":"...","summary":"...","reason":"...","detail_chars":1250}],"missing":[]}
  ```
- **`memory_save`**:
  ```json
  {"id":"mem_123","status":"saved","deduplicated":false}
  ```
  *When coaching hints trigger on candidate saves, up to four `hints` and an `improve` prompt are included.*
- **`memory_feedback`**: Returns `{"id":"mem_123","status":"normal"}` (or `"stale"` if downvoted by owner or two users).
- **`memory_forget`**: Returns `{"id":"mem_123","status":"forgotten"}`.
- **Skill tools**: Return version cards with author, timestamp, vote totals, and markdown content.

## Hermes tools

### Hermes provider tools

In profiles configured with `cairn setup hermes`, Cairn's memory provider supplies all nine tools natively via Python bindings. Calls execute `cairn hook Tool` in the project working directory with identical schemas and limits.

### Hermes gateway tools

In gateway profiles (`cairn setup hermes --gateway`), Cairn exposes native `memory_search`, `memory_save`, and the four skill tools with chat-tag scoping enabled.

See [Projects and memory scopes](../memory/scopes.md#memory-scopes-and-record) and [Retrieval and ranking](../../technical/retrieval.md).
