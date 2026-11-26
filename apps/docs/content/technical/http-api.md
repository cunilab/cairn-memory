---
id: CM-DOC-TECHNICAL-HTTP-API
title: "Server HTTP API"
status: accepted
description: "Server paths, authentication, JSON requests and responses for agents and console clients."
---

# Server HTTP API

Comprehensive REST API endpoint reference, authentication requirements, JSON payloads, and response status codes.

The Cairn server listens on port `3111` by default. All endpoints are prefixed with `/cairn-memory`.

## Authentication and errors

Supply credentials in the `Authorization: Bearer <TOKEN>` header.

| Auth level | Credential required | Usage |
| --- | --- | --- |
| **Public** | None | Health checks, bootstrap, and initial login |
| **Device** | Device token | Agent CLI, hooks, and MCP tools |
| **Web** | Web session token | Web console browser sessions |
| **Admin** | Administrator web session | User administration, audit logs, and Fixes |
| **Either** | Device token or web session | Shared lookups (e.g. `/auth/me`) |

### Standard error format

```json
{"error": "Description of error", "code": "error_code"}
```

- `400`: `validation` (invalid parameters) or `quality` (insufficient title/reason length)
- `401`: `unauthorized` (device token invalid) or `session_required` (missing web cookie)
- `403`: `forbidden` (permission or membership denied)
- `404`: `not_found` (record does not exist or inaccessible)
- `409`: `conflict` (state collision or duplicate constraint)
- `500`: `internal` (database or server failure)

## Health and credentials

| Endpoint | Method | Auth | Parameters | Description |
| --- | --- | --- | --- | --- |
| `/livez` | GET | Public | None | Process liveness check |
| `/health` | GET | Public | None | System status, storage, and semantic search health |
| `/auth/bootstrap` | POST | Public | `email`, `password` | Register initial administrator account |
| `/auth/login` | POST | Public | `email`, `password` | Create new web session |
| `/auth/logout` | POST | Web | None | Revoke active session (204) |
| `/auth/me` | GET | Either | None | Get current account details |
| `/auth/audit` | GET | Web | `limit?`, `before?`, `kinds?` | Retrieve audit events (admins see all) |
| `/devices` | GET | Web | None | List user devices |
| `/devices/register` | POST | Web | `name`, `device_id?` | Create device and receive bearer token |
| `/devices/revoke` | POST | Web | `device_id` | Revoke device access |
| `/device/fixes` | GET | Device | None | Fetch pending automated fixes for device |

## Memory and retrieval

| Endpoint | Method | Auth | Parameters | Description |
| --- | --- | --- | --- | --- |
| `/search` | POST | Device | `query`, `kind?`, `limit?` | Hybrid search across accessible memories |
| `/get` | POST | Device | `ids`, `detail?` | Fetch full memory records and detail |
| `/remember` | POST | Device | `kind`, `title`, `summary`, `reason`, `fields?`, `evidence?` | Save or update memory record |
| `/feedback` | POST | Device | `id`, `vote` (`helpful` \| `wrong`) | Adjust confidence rating |
| `/forget` | POST | Device | `id` | Hide owned memory from recall |

## Skills

| Endpoint | Method | Auth | Parameters | Description |
| --- | --- | --- | --- | --- |
| `/skills/search` | POST | Device | `query`, `limit?`, `offset?` | Search global skill library |
| `/skills/get` | POST | Device | `name_or_id`, `version?` | Retrieve full skill revision |
| `/skills/save` | POST | Device | `name`, `title`, `description`, `body`, `change_note` | Publish or update skill revision |
| `/skills/feedback` | POST | Device | `name_or_id`, `version`, `vote` | Vote on skill revision |

## Projects

| Endpoint | Method | Auth | Parameters | Description |
| --- | --- | --- | --- | --- |
| `/projects` | GET | Web | None | List accessible projects |
| `/projects` | POST | Device | `name`, `project_id?` | Initialize or register project |
| `/projects/{id}` | GET | Web | None | Fetch project details, members, sessions |
| `/projects/{id}/archive` | POST | Web | None | Archive project (pauses agent recall) |
| `/projects/{id}/restore` | POST | Web | None | Restore archived project |

See [Deployment layout](deployment.md) and [Authentication](security.md).
