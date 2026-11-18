---
id: CM-ROADMAP-001
title: "Roadmap — Cairn Memory"
status: draft
---

# Roadmap — Cairn Memory

This roadmap turns AgentMemory's capture and retrieval foundation into Cairn Memory: an AI-managed, person-owned memory layer for people and coding agents. Graphify is a reference for deterministic, explainable code relationships. Collaboration, federation, and advanced graph behavior are later layers, not prerequisites for the first useful product.

The current memory model is the three scopes in [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md), which supersedes the promotion and per-repository personal-memory parts of [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md). Project memory is saved directly and is visible to project members. Personal memory stays private to that person. Global memory is visible on the server. Explicit promotion, invites, per-memory sharing, and team spaces are not the current model. The historical layer text below records what shipped before that decision.

The project should ship in small, reviewable layers. **Every layer must produce a usable Cairn on its own.** A feature is not complete because it exists in code; it needs a clear boundary, a recovery story, and evidence that it behaves safely.

**Current focus:** Layer 6 — Advanced Intelligence and Operations

### ✅ Phase 0 — Product, contract, and memory-autopilot baseline

> **Outcome:** The team agrees that Cairn is AI-managed and person-owned, understands what must be preserved from AgentMemory, and can measure whether automatic memory is useful.

- [x] Define Cairn as an AI-managed, person-owned memory layer.
- [x] Make personal continuity the first product layer; treat collaboration as a later capability.
- [x] Document the layered product model, architecture direction, roadmap, standards, and collaboration boundary.
- [x] Inventory the upstream AgentMemory REST endpoints, MCP tools, hook payloads, CLI behaviors, environment variables, export fields, and lifecycle behavior in the initial compatibility baseline; record exact-schema gaps instead of hiding them.
- [x] Classify interfaces as **must preserve**, **can simplify**, **later**, or **not part of Cairn v1**.
- [x] Define the initial vocabulary: actor, device, space, memory event, memory, cairn, trail, evidence, promotion, revoke, and forget.
- [x] Define automatic capture, retention, correction, privacy, and sharing behavior in plain language.
- [x] Build representative fixtures for session continuity, branch context, sensitivity, deletion, revocation, and offline cases.
- [x] Create a versioned golden corpus and baseline questions for relevance and session resumption.
- [x] Record unresolved product decisions in ADRs instead of silently choosing during implementation.

**Gate:** A reviewer can explain what Cairn saves automatically, why it saves it, who owns it, who can see it, and how a person corrects or forgets it. The artifact-level gate is recorded in [Phase 0 evidence](evidence.md); no runtime implementation is implied.

### Layer 1 — Personal Memory Autopilot

> **Usable outcome:** One person gets useful automatic memory on one machine without manually saving, tagging, or filing anything.

> **Layer 1 status:** The local personal-memory vertical slice is complete: automatic hook capture, lifecycle classification, deterministic consolidation, lessons, corrections, retention sweep, REST, export/import, and the local viewer were implemented and verified here; the CLI and MCP were later rebuilt as device-token network clients (Layer 3). Full upstream registry parity remains a later track.

- [x] Create the smallest Rust workspace and domain model needed for the vertical slice.
- [x] Add the first AgentMemory-compatible REST transport for health, readiness, session lifecycle, observation capture, search, context, forget, and explicit remember.
- [x] Add an MCP stdio transport (`cairn mcp`, a REST client in `crates/client`) with the Layer 1 core tool set and direct capture aliases.
- [x] Preserve the Layer 1 capture, REST, MCP, and export outcomes through local compatibility adapters; full upstream registry parity remains a separate compatibility track.
- [x] Capture sessions and observations with bounded payloads and secret filtering.
- [x] Classify importance, sensitivity, freshness, confidence, and temporary versus durable value.
- [x] Automatically create session summaries, durable memories, and source trails.
- [x] Add lessons and richer lifecycle classification.
- [x] Retain high-value context, expire temporary raw observations, and deterministically compress repeated session noise according to the local Layer 1 policy.
- [x] Implement keyword search and bounded context assembly over durable session context without requiring a remote LLM.
- [x] Implement persisted forget/suppression behavior.
- [x] Implement versioned export/import with idempotent restore and persisted forget tombstones.
- [x] Provide a small agent CLI (`cairn`, a REST client) for session start and end, capture, context, search, forget, lessons, corrections, export, and import.
- [x] Provide a small viewer for recent captures, memory detail, provenance, lifecycle state, diagnostics, search, and forget.
- [x] Provide a portable hook adapter for automatic JSON capture into the REST boundary.

**Gate:** A person can use Cairn for real coding sessions and later recover useful context without deciding what to save.

### Layer 2 — Cross-Session and Branch Continuity

> **Usable outcome:** A new agent session can resume previous work with the right project, branch, files, decisions, tests, and unresolved work.

> **Layer 2 status:** Complete. New memories derive session/worktree/branch/project/global scopes, REST/MCP/CLI retrieval accepts current context, bounded context packs expose scope labels plus omitted-result recovery hints, session summaries expose deterministic next steps and unresolved work, and why-changed retrieval exposes evidence, relationships, stale signals, and contradictions. The structural checks live in `crates/core/src/recall/tests.rs` and `scripts/smoke_full_flow.sh`. They do not measure time or avoided investigation.

- [x] Capture repository, branch, worktree, commit, files, symbols, commands, tests, and services when available.
- [x] Keep session, branch/worktree, project, and user-global scopes distinct for local retrieval.
- [x] Assemble a bounded context pack with source IDs, scope labels, and omitted-result recovery hints.
- [x] Add “what happened last time?”, “why was this changed?”, and “what remains unresolved?” retrieval paths through deterministic handoff and evidence summaries.
- [x] Add deterministic code relationships inspired by Graphify where they improve context quality.
- [x] Add stale-memory and contradiction signals without silently selecting a winner.
- [x] Check why-changed evidence, branch isolation, stale and contradiction signals, and stable repeated investigation in `crates/core/src/recall/tests.rs` and `scripts/smoke_full_flow.sh`. This does not measure time or avoided investigation.

**Gate:** A new session receives relevant context without receiving unrelated branch-local hypotheses, can inspect why a change exists, and sees stale or contradictory evidence instead of an invisible winner. Verified by `scripts/smoke_full_flow.sh` and `why_changed_reports_reason_relationships_staleness_and_contradictions`.

### Layer 3 — Personal Cross-Device Memory

> **Usable outcome:** A person's private memory follows them across trusted devices and agents.

> **Layer 3 status:** Complete for the personal cross-device boundary. File-backed and PostgreSQL modes expose the Cairn `/cairn-memory/` surface; PostgreSQL persists authenticated memory, context retrieval, corrections, export/import, lifecycle diagnostics, audit history, and sync diagnostics. The local outbox purges queued material after device revocation, while the cache and deletion policy is explicit and testable.

- [x] Add local person, agent, and device identity with same-person scoped access.
- [x] Add the first self-hosted account/session authentication adapter and account-authorized device registration.
- [x] Add a PostgreSQL Layer 3 adapter for account sessions, trusted devices, observations, sync events, and cursors.
- [x] Add server-authoritative PostgreSQL persistence for session summaries, explicit memories, forget tombstones, and keyword retrieval.
- [x] Add PostgreSQL parity for context packs, corrections, lifecycle diagnostics, export/import, and advanced retrieval signals.
- [x] Add device registration, rotation, revocation, and last-seen state.
- [x] Make the web console the only place that creates, rotates, or revokes device tokens and manages accounts; agents (`cairn`, `cairn mcp`, the hook) use a device token only ([ADR 0008](adr/0008-self-hosted-account-authentication.md)).
- [x] Add a bounded local spool/outbox and authenticated replay for short outages.
- [x] Make replay idempotent and resumable after process, network, or server failure.
- [x] Define what clients may cache and how revoked or deleted material is purged.
- [x] Add sync lag, rejected events, recovery, and repair information to the viewer.

**Gate:** Memory captured on one trusted device is available exactly once on another without manual export/import, with cursor diagnostics, audit history, and revocation-safe outbox cleanup. Verified by the local and PostgreSQL Layer 3 smoke flows.

### Layer 4 — Project Cairn and Evidence-Backed Promotion

> **Usable outcome:** A repository has durable, branch-aware project knowledge created from verified personal work.

> **Layer 4 status:** Explicit promotion is superseded by [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md). v0.1.4 saves project memory directly; there is no promote step. The earlier slice shipped person-scoped project registration, source evidence, repository matching, secret blocking, idempotent replay, project-scoped retrieval, branch and worktree registration, inspection, merge confirmation, maintenance listing, and linked correction history in both stores. Retiring project entities was not part of that slice.

- [x] Add project, repository, branch, worktree, and decision entities, with file, symbol, handoff, and runbook evidence carried in memory context.
- [x] Keep branch and worktree hypotheses scoped until merge, independent confirmation, or explicit promotion.
- [x] Add source-backed decision cards and evidence trails for files, commits, tests, and commands.
- [x] Add explicit promotion from personal or branch memory into project memory. Superseded by [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md): project memory is saved directly and the promote step is removed.
- [x] Mark project memories stale when later retrieval uses a different commit, and refresh them through explicit verification.
- [x] Add project search and context packs without requiring a team space.
- [x] Keep the viewer focused on inspection, exceptions, and correction rather than manual editing.

**Gate:** A developer can ask why a project decision exists and receive the decision, its evidence, its scope, and its freshness.

### Layer 5 — Collaborative Cairns

> **Usable outcome:** Authorized people and agents can share selected project memory without exposing private personal memory.

> **Layer 5 status:** Invites, per-memory sharing, share policies, and team spaces are superseded by [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md). A member joins through the committed `.cairn/project.json`. Project memory is visible to project members. Personal memory stays private. Global memory is visible on the server. The only roles are owner and member. Delete retains the stored record. Offline purge, encryption, federation, and CRDTs remain unimplemented. The historical share-and-team slice is the checklist below.

- [x] Add project and team spaces, memberships, roles, and permission checks. Team spaces are superseded by [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md); membership and the owner or member role remain.
- [x] Add explicit promote, share, revoke, forget, and delete operations with audit entries. Promote and per-memory share are superseded by [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md) and [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md). Forget and retained delete remain.
- [x] Add explainable share candidates and reviewable exceptions. Superseded by [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md): there is no share-candidate review.
- [x] Allow automatic sharing only when an explicit space policy matches and privacy checks pass. Superseded by [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md): there is no share policy.
- [x] Preserve source actors and trails when memories are summarized or merged.
- [x] Represent contradictions and competing versions instead of silently choosing a winner.
- [x] Add collaboration views for handoffs, decisions, stale facts, conflicts, and revocations.

**Gate:** Two collaborators and their agents can use a shared project cairn while private observations remain private and every shared memory is traceable.

### Layer 6 — Advanced Intelligence and Operations

> **Usable outcome:** Cairn improves retrieval and memory maintenance at scale without weakening ownership or trust boundaries.

> **Layer 6 status:** Semantic retrieval with the versioned local embedding pipeline is implemented in v0.1.4. Exact-search evaluation is still open. The permission-filtered memory link graph and one canvas are implemented. The code graph is open. Durable background jobs are implemented. Declared contradiction handling is implemented. Local or server-side LLM options are open. The operations viewer is partial: the Jobs page is implemented, and the wider health, permissions, audit, and recovery surfaces are open. Multimodal memory is open.

- [x] Add semantic retrieval with a versioned local embedding pipeline (v0.1.4). Exact-search evaluation is still open.
- [x] Add a permission-filtered memory link graph with one-hop recall and one canvas (v0.1.4). The code graph is open.
- [ ] Add bounded graph-assisted retrieval across code and work graphs.
- [x] Add durable background jobs with leases, retries, backoff, quarantine, and dead-letter inspection (v0.1.4).
- [x] Add declared contradiction handling (v0.1.4). Automatic consolidation and staleness decay beyond the retention score are still open. Promotion suggestions are superseded by [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md).
- [ ] Add local or server-side model options while keeping capture and basic recall available during provider failure.
- [x] Show durable jobs on the Jobs page (v0.1.4). Wider operational health, permissions, audit, and recovery surfaces are still open.
- [ ] Add multimodal memory only with explicit privacy, retention, and storage policies.

**Gate:** Automatic processing improves recall and handoffs while provider failures or ambiguous classifications cannot broaden access.

### v1.0 — Trusted personal and project memory

> **Outcome:** Cairn is a stable, inspectable, portable memory source across a person's sessions, agents, projects, and trusted devices, under the three scopes in [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md).

- [x] Personal memory is automatic by default and remains person-owned.
- [x] Session and branch context is scoped, evidence-backed, and retrievable.
- [x] Personal memory synchronizes across trusted devices with recovery and revocation.
- [x] Project memory is the project scope: members can read it, the author or owner can edit it, and explicit promotion is superseded by [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md). Personal memory stays private. Global memory is visible on the server. Provenance, correction, and forget remain.
- [ ] REST, MCP, hook, sync, and export contracts are versioned and governed.
- [x] The viewer explains what Cairn remembers, why it appeared, and how to correct or remove it. Operational health beyond the Jobs page is still open.
- [ ] Security, operations, user guide, launch, and evidence documents are accepted.

## Later

- Federation between independently administered Cairn Memory servers.
- Mesh synchronization and self-hosted sync clusters.
- Public or cross-organization memory spaces.
- Rich collaborative editable memory objects; use CRDTs only where shared concurrent editing is actually required.
- Multi-tenant hosted service, only if a separate product decision approves the security and operational scope.

## Roadmap rules

- A roadmap item is an outcome or a gate, not a task dump.
- Automatic processing must not bypass identity, permissions, or privacy policy.
- Personal memory autopilot precedes cross-device sync; cross-device sync precedes project memory. The older order, in which promotion preceded cross-person sharing, is superseded by [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md). Collaboration of the current member model precedes federation.
- A phase may move only when evidence or a reviewed decision changes the priority.
- A feature that changes a public contract, storage model, trust boundary, or synchronization behavior needs an ADR before it enters an active phase.
