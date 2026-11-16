---
id: CM-SPEC-001
title: "Specification — Collaborative Memory Source"
status: draft
---

# Specification — Collaborative Memory Source

Current project collaboration is defined by [ADR 0013](../adr/0013-shared-project-decisions-and-personal-memory-scopes.md).
Its all-member decisions, explicit publication, personal scopes and online retrieval
supersede the selective-sharing, team-space and project-delivery proposals below.
The current API contract is [project-layer4](../contracts/project-layer4.md).

| Field | Value |
| --- | --- |
| Status | Draft |
| Owner | Cairn Memory maintainers |
| Target | Layers 3–5: personal synchronization, project promotion, and collaboration |
| Related | [Product](../product.md), [Roadmap](../roadmap.md), [Architecture](../architecture.md), [ADR 0002](../adr/0002-collaborative-sync-model.md) |

## Summary

Cairn Memory will extend the AgentMemory foundation from a primarily local memory system into an AI-managed, person-owned memory source for people, agents, and devices. It will automatically capture useful activity, keep personal memory private by default, accept events from trusted clients, and make selected project or team memory available to authorized collaborators and agents as a later layer.

The first collaborative release should build on a useful personal and project memory foundation. It should prioritize clear identity, spaces, permission boundaries, outage-tolerant capture, provenance, and inspectability. It should not attempt to make every captured observation public, require manual curation before personal value appears, or turn every memory into a real-time collaborative document.

## Problem

**User:** Developers, agents, and project teams working across multiple devices and sessions.

**Current behavior:** Memory is local to a runtime or device. People repeat context, manually export notes, copy decisions between tools, or rely on chat and project documents that are not automatically connected to agent recall.

**Problem:** Useful memory does not reliably follow a person across devices or safely flow to collaborators. Automatic capture can also expose private prompts, credentials, personal preferences, or unfinished thoughts if sharing boundaries are unclear.

**Impact:** Teams repeat work, lose handoffs, make inconsistent decisions, and cannot trust an automatic memory system unless they can understand and control its behavior.

## Goal

- [ ] A person can use connected devices and agents while retaining one consistent personal memory view.
- [ ] Cairn can capture, classify, retain, compress, and retrieve useful personal memory without requiring manual save or filing decisions.
- [ ] Authorized collaborators and agents can contribute to and recall shared project memory.
- [ ] Private memory remains private by default and cannot become shared through an implicit summarization step.
- [ ] Offline capture, synchronization, retries, and duplicate delivery are safe and observable.
- [ ] Every shared memory can be traced to its sources, actors, devices, sharing rule, and changes.
- [ ] A user can inspect, correct, revoke, forget, export, or delete memory.

## Scope

| In scope | Out of scope for this specification |
| --- | --- |
| People, agents, devices, workspaces, spaces, memberships, and roles | Public social networking or open discovery |
| Personal, project, team, and organization memory spaces | Federation between independent services |
| Server capture and search with optional client outage buffering | Replacing every source system such as GitHub, Slack, or Jira |
| Authenticated push/pull synchronization with cursors and retries | Peer-to-peer synchronization as the first implementation |
| Automatic classification, deduplication, summarization, retention, promotion candidates, and share candidates | Unrestricted automatic publication of all observations |
| Shared memories with provenance and conflict visibility | Silent last-write-wins conflict resolution |
| Console workflows for permissions, sharing, conflicts, and forget | A general-purpose collaborative document editor |

## User Flows

### 1. Connect a device

1. A person authenticates to Cairn Memory from a new device.
2. The device is registered and receives a device identity and scoped permissions.
3. The client receives permitted context and server events through the authorized API.
4. Any local spool/cache is bounded and reports server acceptance status.

### 2. Capture privately

1. An agent hook sends an observation to the server, or writes it to the edge spool during a short outage.
2. The server filters secrets and records the event in the personal space.
3. The memory autopilot classifies it as useful, sensitive, stale, redundant, temporary, or durable and retains, compresses, or expires it according to policy.
4. Server compression, indexing, and optional embeddings make durable context available to authorized clients.
5. The event is visible to the person's other authorized clients through the server, but not to a project space unless an explicit promotion or policy permits it.

### 3. Share a project memory

1. A worker detects a likely decision, handoff, runbook entry, or project fact in personal or branch memory.
2. The policy engine checks the source space, sensitivity, evidence quality, project membership, and sharing rule.
3. Cairn creates a project candidate by default. If an explicit policy permits it, Cairn may create a share candidate or shared event with its source trail.
4. If the rule is ambiguous, Cairn keeps the result private and creates a reviewable candidate.
5. An authorized person or policy promotes the candidate; source actors, evidence, scope, and sharing reason remain attached.
6. Authorized collaborators and agents receive the shared event through synchronization and can inspect why it was shared.

### 4. Collaborate offline

1. A client captures while the server is unavailable.
2. The client appends the event to its local spool with an idempotency key.
3. When connectivity returns, the client replays the event and receives server acknowledgement.
4. The server deduplicates events and records causal relationships.
5. Non-conflicting events are materialized automatically.
6. Contradictory claims remain visible as competing versions or a conflict record.

### 5. Revoke or forget

1. A user or administrator revokes a device, membership, or memory access grant.
2. The system records an auditable revocation event.
3. Future sync and recall requests stop serving the revoked material.
4. Authorized devices receive a tombstone or purge instruction according to the retention policy.
5. The console shows whether each device acknowledged the revocation or still requires recovery.

## Requirements

### R0 — AI-managed personal memory

- [ ] The default experience does not require a person to manually save, tag, file, or summarize each useful observation.
- [ ] The system classifies importance, sensitivity, confidence, freshness, and temporary versus durable value.
- [ ] Automatic retention, compression, deduplication, and expiry preserve useful context without treating all captured activity as durable memory.
- [ ] Durable memories retain source event IDs and enough evidence to explain why they exist.
- [ ] AI or provider failure does not destroy accepted capture or prevent basic recall.
- [ ] The person can inspect, correct, forget, export, or override automated memory decisions.

### R1 — Identity and spaces

- [ ] The system distinguishes person, agent, device, and service actors.
- [ ] Every event has an actor, device, space, timestamp, and stable event ID.
- [ ] A memory belongs to a personal, project, team, or organization space.
- [ ] Membership and role changes are auditable.
- [ ] Authorization is checked before a memory is returned, indexed for a user, or shared.

### R2 — Automatic capture and classification

- [ ] Existing AgentMemory-compatible hooks can write to the server or an optional edge spool.
- [ ] Capture is idempotent using event identity or an equivalent deduplication key.
- [ ] Privacy filtering occurs before provider calls, synchronization, or shared publication.
- [ ] The system can classify sensitivity, importance, confidence, freshness, and memory value.
- [ ] Provider failure does not discard accepted capture.
- [ ] Ambiguous sharing decisions fail closed into a reviewable candidate state.

### R3 — Synchronization

- [ ] Devices can push events and pull authorized events using durable cursors.
- [ ] Duplicate delivery is safe.
- [ ] Interrupted synchronization resumes without losing acknowledged events.
- [ ] Outage-buffered events are retained locally until server acknowledgement or quarantine.
- [ ] Sync lag, retries, conflicts, and rejected events are visible.
- [ ] Canonical events are stored separately from rebuildable search and graph indexes.

### R4 — Shared memory and provenance

- [ ] A shared memory retains source event IDs and contributing actors.
- [ ] The system records why a memory was shared and under which policy.
- [ ] Derived memory cannot widen visibility beyond its sources without explicit authorization.
- [ ] Contradictory claims are represented and inspectable.
- [ ] Users can promote, correct, supersede, revoke, forget, and delete memory according to their permissions.

### R5 — Agent and human retrieval

- [ ] REST and MCP callers can specify space, project, agent, session, and scope filters.
- [ ] Authorization filtering occurs before ranking and response generation.
- [ ] Results expose enough provenance for an agent or person to judge applicability.
- [ ] The system supports compact context first and deeper source inspection on demand.
- [ ] A user can ask why a result was recalled and what sources contributed to it.

### R6 — Viewer and governance

- [ ] A user can see connected devices, sync state, spaces, memberships, and pending exceptions or share candidates.
- [ ] A user can inspect a memory's trail, visibility, contributors, freshness, and conflicts.
- [ ] A user can correct, revoke, forget, export, or delete memory where authorized.
- [ ] Sensitive operations require confirmation and produce audit entries.
- [ ] The UI distinguishes private, shared, proposed, conflicting, stale, revoked, and deleted states.
- [ ] The viewer does not require users to manually maintain a knowledge base as the normal operating mode.

## Edge Cases

| Case | Expected behavior |
| --- | --- |
| Same event arrives twice | Store one canonical event and acknowledge the duplicate safely |
| Device is offline for an extended period | Keep local capture available; show stale sync state and resume from a cursor later |
| Device is revoked while offline | Stop serving new shared events when it reconnects; record whether purge/revocation was acknowledged |
| Share candidate contains a secret | Redact or quarantine it; do not publish automatically |
| Private and project sources are summarized together | Keep the result private unless an explicit authorized promotion exists |
| Two collaborators make contradictory claims | Preserve both claims, expose provenance, and create a conflict state |
| LLM provider is unavailable | Keep capture and keyword recall working; retry enrichment later |
| User loses a device | Revoke that device without revoking the person's other devices |
| User leaves a team | Remove future access and record the revocation; handle cached material according to the deletion policy |
| Server is unavailable | Clients continue bounded local capture; shared recall waits for the server and outboxes retry later |

## Constraints

- Existing AgentMemory capture, REST, MCP, and export behavior is the compatibility baseline.
- The client must tolerate a short server outage without losing accepted capture; full offline collaborative recall is not required initially.
- Automatic sharing must fail closed when identity, policy, or sensitivity is uncertain.
- Search and embedding indexes are derived and must be rebuildable.
- The first sync model may be centralized or self-hosted; federation is not required for the initial release.
- The system must support audit, retention, deletion, and recovery before cross-person sharing is considered production-ready.

## Success

| Signal | Target |
| --- | --- |
| Personal cross-device recall | A capture made on one trusted device is available exactly once on another after synchronization |
| Shared project recall | An authorized collaborator can recall shared context without receiving private source material |
| Outage reliability | Events captured during a short server outage replay after reconnect without manual re-entry |
| Permission safety | Unauthorized actors receive no private content through search, sync, graph expansion, or summaries |
| Explainability | Every shared memory displays its source trail, contributors, sharing rule, and freshness |
| Recovery | Revocation, deletion, provider outage, duplicate delivery, and partial sync have observable repair paths |

## Open Questions

- [x] **Decision:** The first server uses self-hosted accounts with hashed bearer sessions; OIDC remains a later adapter. See [ADR 0008](../adr/0008-self-hosted-account-authentication.md).
- [ ] **Blocker:** What is the exact deletion guarantee for a client that is offline when access is revoked?
- [ ] **Resolve-as-you-go:** Which data is end-to-end encrypted, and which metadata must remain queryable by the sync plane?
- [x] **Decision:** The first collaborative release requires approval for project promotion by default; automatic sharing is enabled only by explicit policy after evidence supports it.
- [ ] **Resolve-as-you-go:** Which collaborative objects, if any, need CRDT semantics instead of event/version semantics?
- [ ] **Resolve-as-you-go:** Should the server support self-hosted deployment in the first collaborative milestone?
