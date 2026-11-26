---
id: CM-DOC-TECHNICAL-HTTP-API
title: "Server HTTP API"
status: accepted
description: "Server paths, authentication, JSON requests and responses for agents and console clients."
---

# Server HTTP API

The Cairn server listens on port 3111 by default. Routes below start after `/cairn-memory`. If an operator routes the agent API via `https://cairn.example.test/agent`, the full search URL is `https://cairn.example.test/agent/cairn-memory/search`. The console has separate `/api/cairn/*` transport routes, which forward an allowlisted path to `CAIRN_API_URL` and turn its session cookie into a server bearer credential. Use the server address for device-token requests. [Deployment](deployment.md#routing) · [Console proxy](../../../../apps/web/lib/proxy.ts).

## Authentication and errors

Use `Authorization: Bearer TOKEN`. **Device** requires an active device token. **Either** accepts a device token or a web session token. **Web** requires a web session token, and **Admin** also requires the administrator account. Scope, project membership and ownership checks still apply. Send `Content-Type: application/json` with JSON bodies.

Server application errors have `error` and `code` fields. A missing web session can produce this response.

```json
{"error":"Bearer token is required","code":"session_required"}
```

| HTTP status | Code and interpretation |
| --- | --- |
| 400 | `validation` for invalid inputs, or `quality` for a rejected quality constraint |
| 401 | `unauthorized` for failed device authentication, `session_required` for a missing or invalid web session, or `invalid_credentials` for a failed login or incorrect current password |
| 403 | `forbidden` for a permission check |
| 404 | `not_found` for a missing or inaccessible record where the route uses lookup semantics |
| 409 | `conflict` when the requested operation conflicts with saved state |
| 500 | `internal` for a storage or other internal failure handled by the API |

Framework-layer rejections, such as a wrong content type, invalid JSON extraction, or a body over the route's limit, can have a different response shape. A `204` has no body. Optional request arguments below are marked with `?`. Types and path bindings are in the [server handlers](https://github.com/cunilab/cairn-memory/tree/main/crates/server/src/routes) and [error mapping](../../../../crates/server/src/error.rs).

## Health and credentials

| Method and path | Auth | Input | Success response |
| --- | --- | --- | --- |
| `GET /livez` | Public | — | `ok`, `service`. Process liveness. |
| `GET /health` | Public | — | `ok`, `service`, `storage`, `semantic_search`, `bootstrap_required`, `version`, `public_url`. |
| `POST /auth/bootstrap` | Public | `email`, `password` | First account session: `account`, `token`, `expires_at` in epoch seconds. |
| `POST /auth/register` | Admin | `email`, `password` | Created account. |
| `POST /auth/login` | Public | `email`, `password` | Account session, same shape as bootstrap. |
| `POST /auth/logout` | Web | — | 204. Revokes the current session. |
| `GET /auth/me` | Either | — | Account. |
| `GET /auth/audit` | Web | Query: `limit?`, `before?`, `before_id?`, `actor?`, `kinds?` | Audit events. Non-admin callers only see their own. |
| `GET /auth/accounts` | Admin | — | Account array. |
| `POST /auth/accounts/{person_id}/password` | Admin | `password` | 204. Resets the password. |
| `POST /auth/password` | Web | `current_password`, `new_password` | 204. Changes the caller's password. |
| `GET /devices` | Web | — | Array of device views with device details and health information. |
| `POST /devices/register` | Web | `name`, `device_id?`, `person_id?` | `device` and one-time `token`. |
| `POST /devices/revoke` | Web | `device_id` | Revoked device. |
| `POST /devices/rotate` | Web | `device_id` | `device` and the replacement `token`. |
| `GET /device/fixes` | Device | — | Pending fixes for the credential's device. |
| `POST /device/fixes/{id}` | Device | `result`, `reason?` | Empty JSON object. |

Bootstrap and login reject a supplied device bearer. Bootstrap becomes unavailable after initialization. Password changes and device management are explained in [security and privacy](security.md).

## Memory and retrieval

| Method and path | Auth | Input | Success response |
| --- | --- | --- | --- |
| `POST /remember` | Either | Save fields from [MCP](../users/reference/mcp.md#tools), plus `context?`, or `items` with up to three saves | `id`, `status`, `deduplicated`, optional `coaching`, or per-item results. |
| `POST /search` | Either | `query`, `kind?`, `limit?`, `budget?`, `context?`, `debug?` | Compact results and an optional `more` count. If the server has `CAIRN_DEBUG_SCORES=1`, `debug: true` returns a trace. |
| `POST /get` | Either | `ids`, `detail?`, `session_id?` | `memories`, `missing`. |
| `GET /memories` | Either | Query: `limit?`, `cursor?`, `scope?`, `kind?`, `project_id?`, `status?`, `trust?`, `branch?`, `q?` | Memory views and optional `x-next-cursor` header. |
| `GET /memories/{id}` | Either | — | Memory view. |
| `POST /memories/{id}/pin` | Web | `pinned` | `pinned_at`, null for an unpin. |
| `POST /feedback` | Either | `id`, `vote` (`helpful` or `wrong`), `session_id?` | `id`, `status`, optional `stale_cause`. |
| `POST /forget` | Either | `id` | `id` and `status: "forgotten"`. |
| `GET /memories/export` | Web | — | Downloadable JSON memory export. |
| `POST /memories/import` | Web | Export-shaped JSON, optional query `dry_run`, 16 MiB body limit | Import report with new, duplicate and invalid results. |
| `GET /graph` | Either | Query: `scope?`, `project_id?`, `relation?`, `cap?` | Graph nodes and edges. |

This body searches for the decision saved in the Quickstart.

```json
{"query":"disposable PostgreSQL schema","kind":"decision","limit":5}
```

The CLI and MCP shape compact save receipts and errors for their consumers. The server receipt nests its optional coaching fields.

## Project and capture

| Method and path | Auth | Input | Success response |
| --- | --- | --- | --- |
| `GET /projects` | Either | — | Project list with roles, counts and activity. |
| `POST /projects/init` | Either | `repository`, `project_id?`, `name?`, `confirm_register?` | `project_id`, `repository`, `name`, `created`. |
| `POST /projects/{project_id}/rename` | Web | `name` | Updated project. |
| `POST /projects/{project_id}/archive` | Web | — | Archived project. |
| `POST /projects/{project_id}/restore` | Web | — | Restored project. |
| `POST /projects/{project_id}/ownership` | Web | `person_id` | 204. |
| `GET /projects/{project_id}/memories` | Either | Query: `limit?`, `cursor?` | Project memories with optional `x-next-cursor`. |
| `GET /projects/{project_id}/members` | Either | — | Member records with person, email, role and activity fields. |
| `GET /projects/{project_id}/branches` | Either | — | Branch summaries. |
| `GET /projects/{project_id}/conflicts` | Either | — | Project memory conflicts. |
| `GET /projects/{project_id}/sessions` | Either | Query: `days?` | Session reports. |
| `POST /observe` | Device | `event_id`, `session_id`, `kind` (prompt, edit, command or assistant), `content`, `ok?`, `context?`. Optional health metadata: `hook_error_class`, `hook_error_at`, `outbox_depth`. | `accepted`, `duplicate`, `event_id`, `project_attachment`. |
| `POST /session/repo-state` | Device | Repository state report | Repository-state receipt. |
| `POST /session/lookup` | Device | `session_id`, `kind`, `query?`, `result_ids?`, `used_ids?`, `hit?` | 204. |
| `POST /session/end` | Either | `session_id`, `nudge_asked?`, `nudge_saved?`, `chat?` | `status`: `queued` or `empty`. |
| `POST /session/checkpoint` | Either | Same reference as session end | `status`: `stored` or `unchanged`. |
| `POST /session/brief` | Either | `session_id?`, `context?`, `recent_files?`, `extended_brief?` | `max_bytes`, `brief`, and applicable instructions, check and project fields. |
| `POST /session/topic` | Either | `path`, `session_id?`, `context?` | File-topic response. |
| `POST /session/command-recall` | Either | `session_id`, `command`, `project_id?`, `error_tail?`, `chat?` | `text`, which can be empty. |
| `POST /session/prompt-recall` | Either | `session_id`, `prompt`, `context` | `text`, with `budget_spent` when available. |

The Rust [capture types](../../../../crates/core/src/capture/observation.rs) define observation input and work context. A tool's work context can include project, repository, branch, worktree, commit and bounded entity lists. [Hook protocol](hooks/index.md#events) · [Project rules](../users/memory/scopes.md#init-and-membership).

## Skills, instructions and operator routes

| Method and path | Auth | Input | Success response |
| --- | --- | --- | --- |
| `POST /skills/search` | Either | `query`, `limit?`, `offset?`, `archived?` | `results`, `next_offset`. |
| `GET /skills` | Web | Same fields as search in the query string | `results`, `next_offset`. |
| `POST /skills/get` | Either | `name_or_id`, `version?` | Skill detail including body and version. |
| `POST /skills/save` | Either | `name`, `title`, `description`, `body`, `change_note` | Saved version card. |
| `POST /skills/create` | Web | Same skill draft as save | Saved version card. |
| `POST /skills/feedback` | Either | `name_or_id`, `version`, `vote`, `note?` | Version card. |
| `GET /skills/{key}` | Web | Query: `version?` | Version detail. |
| `GET /skills/{key}/history` | Web | — | Version history. |
| `POST /skills/{key}/rollback` | Web | `version`, `change_note` | New version detail. |
| `POST /skills/{key}/archive` | Admin | — | 204. |
| `POST /skills/{key}/restore` | Admin | — | 204. |
| `GET /skills/suggestions` | Web | — | Visible procedure suggestions. |
| `POST /skills/suggestions/{memory_id}/create` | Web | — | Created skill card. |
| `POST /skills/suggestions/{memory_id}/dismiss` | Web | — | 204. |
| `GET /agent-instructions` | Either | — | Global instruction settings. |
| `POST /agent-instructions` | Admin | `session_instruction?`, `stop_nudge?`, `chat_instruction?` | Saved global settings. |
| `GET /projects/{project_id}/agent-instructions` | Either | — | Project instruction settings. |
| `POST /projects/{project_id}/agent-instructions` | Web | `session_instruction?`, `stop_nudge?` | Saved settings. Requires project owner or administrator. |
| `GET /stats` | Web | Query: `tz?`, `days?` | Console statistics. |
| `GET /stats/attention` | Web | — | Console attention counts. |
| `GET /admin/jobs` | Admin | — | Job snapshot. |
| `GET /admin/fixes` | Admin | Query: `status?`, `detector?`, `group?`, `cursor?` | Fix list. History can include `x-next-cursor`. |
| `GET /admin/fixes/count` | Admin | — | `count` of pending fixes. |
| `GET /admin/fixes/{id}` | Admin | — | Fix detail. |
| `POST /admin/fixes/{id}/apply` | Admin | `fingerprint` | Fix result. Stale fingerprints or incompatible status can return 409. |
| `POST /admin/fixes/{id}/ignore` | Admin | — | Fix result. |
| `POST /admin/fixes/{id}/copied` | Admin | — | Fix result. |
| `POST /admin/fixes/{id}/done` | Admin | — | Fix result. |
| `GET /admin/searches` | Admin | — | Recent redacted search events. |
| `POST /admin/memories/{id}/rewrite` | Admin | `title`, `summary`, `reason`, `fields?` | `memory_id`, `superseded`. |

[MCP tool schemas and limits](../users/reference/mcp.md#tools) · [Admin operations and privacy](security.md#admin-api).
