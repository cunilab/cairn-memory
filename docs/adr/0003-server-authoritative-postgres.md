---
id: CM-ADR-0003
title: "ADR-0003: Make the server authoritative and use PostgreSQL for canonical memory"
status: draft
---

# ADR-0003: Make the server authoritative and use PostgreSQL for canonical memory

- **Date:** 2026-10-25
- **Milestone:** Layers 1, 3, and 6 — personal memory, synchronization, and retrieval infrastructure
- **Related:** [Product](../product.md), [Roadmap](../roadmap.md), [Architecture](../architecture.md), [Graph-assisted retrieval](../specs/graph-assisted-retrieval.md)

## Context

Cairn Memory must serve multiple people, agents, projects, and devices. A local database on each client is useful for capture buffering, but it creates competing sources of truth for permissions, shared memory, user management, audit history, and retrieval. The product also needs web administration, login, admin-created users, MCP token management, upgrades, and server-side background processing.

The server needs one database that can handle relational permissions and scopes, full-text search, embeddings, graph relationships, transactions, audit history, and background jobs without introducing several independent storage systems at the beginning.

## Decision

1. The Cairn Memory server is the canonical source of truth for accepted observations, memory events, memories, users, organizations, projects, spaces, permissions, tokens, audit records, jobs, and conflicts.
2. PostgreSQL is the canonical server database.
3. PostgreSQL full-text search owns lexical retrieval for the first server implementation.
4. `pgvector` owns server-side embedding storage and vector retrieval when semantic retrieval is enabled. The first usable personal-memory layer must work with keyword and metadata retrieval; later production semantic retrieval uses a versioned embedding model that may run locally on the server, while LLM generation remains optional.
5. Graph nodes, edges, provenance, and code relationships are stored in PostgreSQL tables and queried with bounded relational traversal. A separate graph database is not required initially.
6. Clients may keep a bounded local spool, retry queue, and recent cache. These are delivery aids, not canonical memory stores.
7. Every accepted client write has an idempotency key. Replays are safe and return the original acceptance result.
8. Embeddings include model ID, model version, dimensions, normalization, source text fingerprint, and created-at metadata. Changing the model creates a re-embedding job; it does not mutate vectors invisibly.
9. LLM enrichment, summaries, promotion proposals, and graph semantic labeling are asynchronous jobs. Capture, PostgreSQL persistence, lexical indexing, and deterministic code-graph extraction do not require an LLM key.
10. The web UI is the primary administration surface for users, projects, tokens, permissions, jobs, upgrades, and memory governance. The CLI remains for bootstrap, deployment, migration, and emergency recovery.

## Consequences

### Positive

- One permissioned source of truth serves many people, agents, and devices.
- PostgreSQL transactions cover memory, permissions, audit, jobs, and events together.
- PostgreSQL FTS and pgvector allow hybrid retrieval without a second mandatory search service.
- The relational model is a good fit for organizations, projects, branches, sessions, users, and audit trails.
- A local edge outage strategy stays simple: spool, retry, idempotent replay.
- The server can expose a consistent web administration experience.

### Costs and accepted risks

- The server is required for shared recall and canonical writes; outage tolerance is not full offline collaboration.
- PostgreSQL and pgvector must be operated, backed up, upgraded, and monitored.
- Approximate vector indexes can return fewer filtered results; authorization and tenant filtering need explicit query/index design.
- Re-embedding is a real migration operation, not a configuration toggle.
- Very large graph workloads may eventually require a specialized graph index or graph service, but that decision is deferred until measurements justify it.

## Alternatives considered

### Client-local SQLite as the canonical store

Rejected because multi-user permissions, shared memory, admin operations, and server-side audit would require conflict-prone synchronization between canonical stores.

### PostgreSQL plus a separate vector database from day one

Rejected because it adds operational complexity before measuring whether pgvector meets the retrieval workload. A separate vector system can be added behind the search interface later.

### PostgreSQL plus a separate graph database from day one

Rejected because the first graph needs bounded code and memory relationships, not an unbounded graph platform. Store graph facts relationally first and measure.

### LLM-required ingestion

Rejected because provider outages, cost, and privacy must not prevent capture or basic retrieval.

## Amendment 2026-11-15: CLI scope and replay results

- Decision 10: the web console is the only place to manage accounts, devices, and tokens. The `cairn` CLI is an agent client with a device token (plus the one-time `cairn setup`), not an administration tool; see the [second 2026-11-14 amendment to ADR 0008](0008-self-hosted-account-authentication.md#amendment-2026-11-14-second-the-cli-and-mcp-are-device-token-network-clients). Server startup and migration stay in `cairn-server`.
- Decision 7: an identical replay returns the original acceptance result. A replay of the same event id with a changed session, content, role, or source returns `409 Conflict` instead; see the [2026-11-15 amendment to ADR 0007](0007-layer3-local-trusted-device-slice.md#amendment-2026-11-15-a-changed-or-out-of-scope-replay-is-a-conflict).

This ADR keeps its `draft` status.
