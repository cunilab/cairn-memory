---
id: CM-ADR-0002
title: "ADR-0002: Use scoped event-based collaboration synchronization"
status: draft
---

# ADR-0002: Use scoped event-based collaboration synchronization

- **Date:** 2026-10-25
- **Milestone:** Phase 0 and Layers 3–5 — product, personal sync, and collaboration boundary
- **Related:** [Product](../product.md), [Roadmap](../roadmap.md), [Architecture](../architecture.md), [Collaborative memory specification](../specs/collaborative-memory.md)

## Context

Cairn Memory needs to extend a strong single-device memory foundation into a person-owned source that works across sessions and devices before it works across people. Cross-person collaboration introduces new trust boundaries: identities, devices, spaces, permissions, sharing, revocation, deletion, outage buffering, duplicate delivery, stale facts, and conflicting claims.

The system should feel automatic, but automatic processing must not become automatic disclosure. It must be possible to answer who created a memory, which device observed it, why it was retained or promoted, who can see it, what changed it, and how access can be revoked.

The system should tolerate short server outages. A client should capture locally into a durable spool and replay after reconnect, but the client should not become a competing collaborative source of truth. Synchronization should resume after failure and should not duplicate events.

## Decision

Cairn Memory will use a **scoped, event-based synchronization model**:

1. A centralized or self-hosted Cairn server owns canonical memory, identity, spaces, permissions, events, indexes, and jobs in PostgreSQL when server mode is enabled; the first useful layer may run as one local process.
2. A client may run an optional edge spool/cache for hook capture, retry, and recent context, but the client is not a second collaborative database.
3. The server is the first collaboration target. Peer-to-peer federation is deferred until identity, deletion, revocation, and recovery are proven.
3. Every actor is explicit: person, agent, device, or service. Events record the creating actor and device, and authorization records the actor that permitted a transition when those differ.
4. Every event belongs to a space. The initial spaces are personal, project, team, and organization. A future federated space is a separate trust model.
5. Raw observations and state transitions are append-only events. Memories, summaries, embeddings, search indexes, graph projections, and counts are materialized views that retain source event IDs.
6. Client replay uses authenticated delivery with event IDs, idempotency keys, acknowledgements, retries, deduplication, tombstones, and causal metadata. Server-to-client live updates use SSE initially; pull cursors may be added later.
7. Visibility is non-escalating by default: a derived memory cannot become more visible than its least-visible source without an explicit authorized share operation.
8. Automatic processing may classify, deduplicate, summarize, retain, expire, and propose promotion or sharing. Automatic sharing is a later capability allowed only when an explicit space policy matches and privacy checks pass; otherwise the result remains private or enters a reviewable candidate state.
9. Retrieval applies authorization and space filtering before ranking, embedding, graph expansion, or response generation.
10. Conflicting claims are preserved as competing versions or explicit contradiction relations. Last-write-wins is not the default for memory facts.
11. CRDTs are reserved for explicitly collaborative editable objects, such as a shared working agreement or runbook section. Atomic memory events use the event log and versioned projections instead of turning every memory into a collaborative document.
12. Matrix-like event/cursor concepts and ActivityPub-like federation concepts may inform future interoperability, but neither protocol is adopted as the core storage or trust model in the first release.

## Consequences

### Positive

- A client remains able to capture during a short outage and can replay later without becoming canonical.
- Cross-device and cross-person access has explicit identity and scope.
- Provenance, conflict, sharing, and deletion decisions can be inspected.
- Duplicate delivery and retries can be made safe with event IDs and cursors.
- Derived indexes can be rebuilt independently on the server; optional clients may maintain only bounded caches.
- The system can add federation later without making the first release depend on it.

### Costs and accepted risks

- The server is a required operational dependency for shared recall; outage tolerance does not provide full offline collaboration initially.
- Revocation and deletion across already-synchronized devices are difficult and need explicit evidence.
- Event storage and audit history require retention, compaction, and privacy policies.
- Automatic sharing needs a policy engine, quarantine state, and a human review experience for ambiguous cases.
- Conflict resolution is a product problem as well as a storage problem; not every contradiction can be resolved algorithmically.
- Centralized or self-hosted synchronization creates a trust and availability dependency until a federation model exists.

## Alternatives considered

### Peer-to-peer CRDT synchronization first

Rejected as the first target because it makes identity, discovery, revocation, abuse handling, deletion, and recovery harder before the product's collaboration semantics are validated. CRDTs remain appropriate for selected collaborative editable objects.

### Matrix as the storage and synchronization protocol

Rejected for the first implementation because Cairn Memory needs structured memory semantics, source visibility, agent/device identity, authorization-aware retrieval, and data lifecycle rules beyond a general real-time room protocol. Matrix-like cursors and event concepts remain useful references.

### ActivityPub as the core collaboration protocol

Rejected for the first implementation because ActivityPub is optimized for federated actor activities and social graph interoperability, while Cairn Memory needs private, scoped, searchable, revocable project memory. It may be useful for a future federation adapter.

### A single centralized database with no local edge spool

Rejected because hooks need bounded capture behavior during short server outages. The edge spool is intentionally limited and must not become a second source of truth.

### Last-write-wins records

Rejected for memory facts because silently overwriting a teammate's decision or an agent's evidence destroys provenance and hides disagreement. Explicit versions and contradictions are safer.

## Reference material

- [Matrix Specification](https://spec.matrix.org/latest/) — useful reference for event synchronization, cursors, state deltas, and future federation thinking.
- [ActivityPub Recommendation](https://www.w3.org/TR/activitypub/) — useful reference for actor-based federation, but not the initial private memory protocol.
- [Yrs Rust CRDT](https://docs.rs/yrs/latest/yrs/) — a possible future implementation for explicitly collaborative editable objects, not the default memory-fact model.
