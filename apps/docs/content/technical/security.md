---
id: CM-REF-005
title: "Accounts and authentication"
status: draft
description: "Account and device credentials, project visibility and data handling."
---

# Accounts and authentication

Authentication architecture, device token verification, administrative APIs, and secret redaction.

## Accounts and sessions

- **Bootstrap**: The first registered account becomes the sole administrator. Open signups are disabled.
- **Humans**: Web console users authenticate via cookie-backed sessions. Passwords are encrypted with Argon2. Resetting a password revokes active web sessions without invalidating device tokens.
- **Role separation**: Account administration, audit viewing, and Fixes management require an administrator session.

## Device tokens

- **Agents**: Coding agents authenticate via bearer tokens generated under **Settings → Devices**.
- **Scope**: Device tokens have read/write access to memories and skills according to the owning user's project memberships.
- **Restricted surface**: Device tokens cannot manage accounts, create devices, or view account-wide audit logs.
- **Revocation**: Revoking a device immediately invalidates its bearer credential.

## Admin API

Administrative endpoints under `/cairn-memory/admin/` require an active administrator web session:

- **`GET /fixes`**: Lists open and resolved system issues.
- **`POST /fixes/{id}/apply`**: Applies automated fixes (fingerprint validation protects against stale state).
- **`GET /searches`**: Returns redacted recent query logs.
- **`POST /memories/{id}/rewrite`**: Allows administrators to redact leaked secrets across a memory's revision chain.

## Privacy and data

- **Secret redaction**: Pre-storage redaction automatically masks API keys, bearer tokens, private keys, and passwords.
- **Audit trail**: Account creation, project joins, ownership transfers, and memory rewrites produce permanent audit events.
- **Scope isolation**: Personal memories belong strictly to the owner; project memories require membership in `.cairn/project.json`.

See [Project membership](../users/memory/scopes.md#init-and-membership) and [Deployment limits](deployment.md#limits).
