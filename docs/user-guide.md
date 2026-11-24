---
id: CM-GUIDE-004
title: "Cairn Memory user guide"
status: draft
---

# Cairn Memory user guide

Set `CAIRN_SESSION_ORIGIN=automation` in the environment of a scheduled run, worker, or subagent. Cairn still captures that session and saves its distilled notes, and it does not ask to save at Stop and does not replace the handoff for that checkout. A person's own session in the same checkout is unchanged.

## Fixes

Use `cairn fixes` to list this device's queued fixes, or `cairn fixes apply [fx_id]` to apply all of them or one by id. The `init` and `doctor` commands also try queued fixes first; agent commands, hooks, and MCP do not. A committed `.cairn/project.json` joins the person using it to that project, and the join is recorded in the audit log.

Admins can review Fixes in the web console. Pending lists fixes awaiting a decision, queued computer work, and prompts awaiting proof; History shows completed and ignored items. Searches shows the last 14 days of search phrases with secrets redacted. Server fixes change data only after Apply; computer fixes are queued for the device; agent fixes provide a prompt to copy. Secret removal is permanent: rotate the exposed secret immediately. There is no undo.

An admin may rewrite a memory from its detail sheet. Cairn preserves the owner, scope and project, retains the superseded version, and shows the owner that an admin changed it.

The admin web-session API is under `/cairn-memory/admin/`. `GET fixes` accepts optional `status` and `detector`; `GET fixes/{id}` returns a fix and its fingerprint. `POST fixes/{id}/apply` takes `{"fingerprint":"..."}` and returns the updated fix; a stale fingerprint or incompatible status returns 409. `POST fixes/{id}/ignore`, `/copied`, and `/done` take an empty object and return the updated fix. `GET searches` returns recent redacted search phrases. `POST memories/{id}/rewrite` takes `title`, `summary`, `reason`, and optional `fields`, and returns `memory_id` and `superseded`. Device tokens are rejected with 401; non-admin web sessions receive 403.
