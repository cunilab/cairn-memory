---
id: CM-ADR-0007
title: "ADR-0007: Prove trusted-device sync before PostgreSQL and hosted authentication"
status: draft
---

# ADR-0007: Prove trusted-device sync before PostgreSQL and hosted authentication

- **Date:** 2026-10-26
- **Milestone:** Layer 3 — Personal Cross-Device Memory
- **Related:** [Roadmap](../roadmap.md), [Collaborative memory](../specs/collaborative-memory.md), [Layer 3 sync contract](../contracts/sync-layer3.md), [ADR 0002](0002-collaborative-sync-model.md)

## Context

Layer 3 needs a real cross-device behavior, but the current implementation is
still a deliberately small file-backed Rust prototype. Introducing PostgreSQL,
OIDC, key rotation, encrypted caches, and a full edge client at once would make
it difficult to prove the core behavior and would overstate the security model.

## Decision

1. Add person-scoped device records with active/revoked status, last-seen time,
   and a durable sync cursor.
2. Use an opaque local prototype bearer token for the first trusted-device
   boundary; explicitly label it as non-production authentication.
3. Store accepted sync observations as person-scoped, cursor-addressable events.
4. Make push idempotent by `eventId`; make pull resumable by client cursor.
5. Add token rotation and same-person revocation before any sharing model.
6. Keep outage buffering in a bounded local JSON outbox that removes entries
   only after server acknowledgement and redacts common secrets locally.
7. Defer PostgreSQL, token hashing, OIDC, encrypted caches, cross-device purge
   guarantees, and production deployment until the local sync behavior is proven.

## Consequences

- The project has a real, testable cross-device flow without adding a database
  or authentication provider dependency.
- Duplicate delivery, cursor resume, token rotation, and revocation are visible
  and testable.
- The local token implementation must not be presented as production security.
- The next Layer 3 slice can replace the storage and token adapter while
  preserving the sync contract and acceptance flow.

The next slice is now implemented by `cairn-storage`: PostgreSQL-backed
account sessions, hashed device tokens, canonical observations, sync events,
and cursor persistence preserve this contract through the server's
`--database-url` mode. Encrypted caches and purge guarantees remain deferred.
