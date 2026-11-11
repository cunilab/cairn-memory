---
id: CM-ADR-0008
title: "ADR-0008: Use self-hosted accounts with hashed bearer sessions"
status: draft
---

# ADR-0008: Use self-hosted accounts with hashed bearer sessions

- **Date:** 2026-11-03
- **Milestone:** Layer 3 — Personal Cross-Device Memory
- **Related:** [Roadmap](../roadmap.md), [ADR 0003](0003-server-authoritative-postgres.md), [ADR 0007](0007-layer3-local-trusted-device-slice.md), [Layer 3 authentication contract](../contracts/auth-layer3.md)

## Context

The local trusted-device slice uses opaque prototype device tokens and an
unauthenticated bootstrap route for device registration. The next Layer 3
slice needs a person-level identity boundary before a device can be attached to
that person. The first deployment target is self-hosted, and PostgreSQL is the
planned canonical store.

External OIDC would add an identity-provider dependency before the server's
basic account and device ownership behavior is proven. Supporting both models
at once would broaden onboarding, token, recovery, and test surfaces before
the personal-memory path is complete.

## Decision

1. The first production-oriented identity model is self-hosted accounts with a
   normalized email identifier and a password.
2. Passwords are stored only as Argon2id password hashes. The raw password is
   never persisted or included in responses.
3. Account sessions use opaque random bearer tokens. Only SHA-256 token hashes
   are persisted; sessions expire after 30 days and can be revoked.
4. The first account is created through a one-time bootstrap operation. Login,
   logout, and current-account inspection use the account bearer token.
5. Device registration requires an active account token and the requested
   `personId` must match that account. Device tokens remain a separate bearer
   credential for agent sync, rotation, and revocation.
6. OIDC, password recovery, multi-factor authentication, account invitations,
   and hosted identity federation remain later work.

## Consequences

- Account ownership is checked before a device is registered.
- The file-backed adapter can exercise the authentication contract without
  requiring a database. The `cairn-storage` adapter provides the async
  PostgreSQL account, device, observation, and sync-event boundary.
- Self-hosted operators own password and account recovery responsibilities.
- Device tokens and account sessions have separate scopes and revocation paths.
- A future OIDC adapter can map provider subjects to the same `person_id`
  without changing memory or sync ownership semantics.

## Security boundary

This ADR defines the first account contract and storage shape. The PostgreSQL
adapter is the first server-authoritative Layer 3 deployment path; a complete
production deployment still needs secret rotation, rate limiting, password
recovery, audit records, encrypted caches, advanced retrieval parity, and
offline purge guarantees.
