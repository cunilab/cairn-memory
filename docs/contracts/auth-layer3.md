---
id: CM-CONTRACT-008
title: "Layer 3 Self-Hosted Account Authentication"
status: draft
---

# Layer 3 Self-Hosted Account Authentication

This contract defines the first person-level authentication boundary. It is
separate from device bearer tokens used by sync clients.

## Account bootstrap

`POST /cairn-memory/auth/bootstrap` creates the first account only:

```json
{
  "email": "owner@example.test",
  "password": "correct horse battery staple"
}
```

The response contains an account record, an opaque session token, and its
expiry. Email identifiers are normalized to lowercase. Passwords must be at
least 12 bytes and are never returned or stored in plaintext.

After bootstrap, another bootstrap request fails with `400`.

## Account sessions

| Operation | Endpoint | Authorization |
| --- | --- | --- |
| Login | `POST /cairn-memory/auth/login` | None; email and password body |
| Logout | `POST /cairn-memory/auth/logout` | Account bearer token |
| Current account | `GET /cairn-memory/auth/me` | Account bearer token |
| Audit history | `GET /cairn-memory/auth/audit` | Account bearer token; PostgreSQL mode |

Account tokens are sent as:

```text
Authorization: Bearer <account-session-token>
```

The file-backed and PostgreSQL adapters store only a hash of the opaque token.
Sessions expire after 30 days and logout makes the token unusable immediately.
Invalid account credentials and revoked or expired sessions do not return
account data.

## Device ownership

`POST /cairn-memory/devices/register` now requires an active account bearer
token. The request's `personId` must equal the authenticated account's
`person_id`. The response still contains a separate opaque device token for
the existing authenticated sync contract.

Device sync endpoints continue to authorize with device tokens. Account logout
does not revoke device sync credentials; device rotation and revocation remain
the explicit controls for agent/device access.

## Limitations

The file-backed compatibility adapter remains available for local Layer 1/2
flows. The `cairn-storage` adapter and `--database-url` server mode provide the
PostgreSQL account, session, device, observation, and sync-event boundary.
Authentication attempts are rate-limited per normalized account identifier in
the running server. PostgreSQL records metadata-only audit events for account
bootstrap, login, logout, and trusted-device/sync operations; passwords,
tokens, and memory content are never written to the audit table. Password
recovery, MFA, and OIDC remain separate adapters because they require an
email/identity provider decision beyond the self-hosted Layer 3 boundary.

## Evidence

- `crates/core/src/lib.rs` implements password hashing, session hashing,
  expiration, persistence, and logout.
- `crates/server/src/lib.rs` exposes bootstrap, login, logout, current-account,
  account-authorized device registration, and PostgreSQL Layer 3 routes.
- `crates/storage/src/lib.rs` implements the async PostgreSQL account, device,
  and sync repository.
- `migrations/0001_layer3_personal_memory.sql` defines the PostgreSQL boundary.
- `migrations/0002_layer3_audit.sql` defines metadata-only operational audit history.
- `crates/storage/tests/postgres.rs` records the provisioned-database flow.
