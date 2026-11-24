---
id: CM-REF-005
title: "Accounts and authentication"
status: draft
---

# Accounts and authentication

## Accounts and sessions

The first account bootstrapped through the web is the only admin. The admin creates additional accounts; there is no open signup. Humans sign in with a web session. Passwords and bearer credentials are stored as hashes. The web session is required for account and device management; a device token cannot perform those operations. Resetting a password revokes web sessions, not device credentials.

## Device tokens

A person creates, rotates and revokes their device tokens in the web console. A token is shown at creation and is used by the CLI, hook and MCP as that person's agent credential. `cairn setup <server-url>` reads the token from stdin and writes the selected config file. Revoking a device prevents further agent access. See [CLI and MCP](cli-and-mcp.md) for configuration and URL rules.

## Admin API

Admin routes require an admin web session; device tokens are rejected. Under `/cairn-memory/admin/`, `GET fixes` accepts `status` and `detector`, and `GET fixes/{id}` returns the fix. `POST fixes/{id}/apply` accepts a fingerprint and rejects a stale fingerprint or incompatible status with 409. The `ignore`, `copied` and `done` actions take an empty object. `GET searches` returns recent redacted search phrases. `POST memories/{id}/rewrite` takes title, summary, reason and optional fields and returns the new and superseded IDs. Secret-removal fixes are permanent.
