---
id: CM-REF-005
title: "Accounts and authentication"
status: draft
description: "Account and device credentials, project visibility and data handling."
---

# Accounts and authentication

## Accounts and sessions

The first account bootstrapped through the web is the only admin. The admin creates additional accounts; there is no open signup. Humans sign in with a web session. Passwords and bearer credentials are stored as hashes. The web session is required for account and device management; a device token cannot perform those operations. Resetting a password revokes web sessions, not device credentials.

## Device tokens

A person creates, rotates and revokes their device tokens in the web console. A token is shown at creation and is used by the CLI, hook and MCP as that person's agent credential. `cairn setup <server-url>` reads the token from stdin and writes the selected config file. Revoking a device prevents further agent access. See [CLI and MCP](../users/reference/mcp.md) for configuration and URL rules.

## Admin API

Admin routes require an admin web session; device tokens are rejected. Under `/cairn-memory/admin/`, `GET fixes` accepts `status`, `detector`, and `group` (`open` or `history`). History also accepts `cursor` and returns the next page in `x-next-cursor`. `GET fixes/{id}` returns the fix. `POST fixes/{id}/apply` accepts a fingerprint and rejects a stale fingerprint or incompatible status with 409. The `ignore`, `copied` and `done` actions accept a request without a body. `GET searches` returns recent redacted search phrases. `POST memories/{id}/rewrite` takes title, summary, reason and optional fields and returns the new and superseded IDs. Secret redaction covers the live memory's title, summary, reason, and kind fields. A secret can remain in evidence or an older version.

## Privacy and data

Capture and saves are redacted before storage using best-effort patterns; check sensitive data before sharing it. Raw observations are retained for processing, not returned by memory search, and expire after 30 days; captures from a completed, distilled session can be removed after seven days. Forget hides a retained record from normal retrieval. Export is available in the console. Administrators can inspect redacted recent search phrases and rewrite memories; rewrites preserve the original and show that an admin changed it. Scope and membership determine who can see project or global memory.

Project IDs grant membership to authenticated accounts. Protect the project file, and verify membership before sharing sensitive project memory. See [project membership](../users/memory/scopes.md#init-and-membership). Serve the console and agent endpoint over TLS outside loopback. Review [deployment limits](deployment.md#limits) before exposing routes.
