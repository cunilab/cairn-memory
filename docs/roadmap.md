---
id: CM-ROADMAP-001
title: "Roadmap — Cairn Memory"
status: draft
---

# Roadmap — Cairn Memory

This roadmap turns AgentMemory's capture and retrieval foundation into Cairn Memory: an AI-managed, person-owned memory layer for people and coding agents. Graphify is a reference for deterministic, explainable code relationships. Collaboration, federation, and advanced graph behavior are later layers, not prerequisites for the first useful product.

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

> **Layer 1 status:** The local personal-memory vertical slice is complete: automatic hook capture, lifecycle classification, deterministic consolidation, lessons, corrections, retention sweep, REST, MCP, export/import, CLI, and the local viewer are all implemented and verified. Full upstream registry parity, authentication, PostgreSQL, and the Next.js console remain later tracks.

- [x] Create the smallest Rust workspace and domain model needed for the vertical slice.
- [x] Add the first AgentMemory-compatible REST transport for health, readiness, session lifecycle, observation capture, search, context, forget, and explicit remember.
- [x] Add a standalone MCP stdio transport with the Layer 1 core tool set and direct capture aliases.
- [x] Preserve the Layer 1 capture, REST, MCP, and export outcomes through local compatibility adapters; full upstream registry parity remains a separate compatibility track.
- [x] Capture sessions and observations with bounded payloads and secret filtering.
- [x] Classify importance, sensitivity, freshness, confidence, and temporary versus durable value.
- [x] Automatically create session summaries, durable memories, and source trails.
- [x] Add lessons and richer lifecycle classification.
- [x] Retain high-value context, expire temporary raw observations, and deterministically compress repeated session noise according to the local Layer 1 policy.
- [x] Implement keyword search and bounded context assembly over durable session context without requiring a remote LLM.
- [x] Implement persisted forget/suppression behavior.
- [x] Implement versioned export/import with idempotent restore and persisted forget tombstones.
- [x] Provide a small CLI path for init, capture, session end, context, search, forget, lessons, corrections, export, and import.
- [x] Provide a small viewer for recent captures, memory detail, provenance, lifecycle state, diagnostics, search, and forget.
- [x] Provide a portable hook adapter for automatic JSON capture into the REST boundary.

**Gate:** A person can use Cairn for real coding sessions and later recover useful context without deciding what to save.

### Layer 2 — Cross-Session and Branch Continuity

> **Usable outcome:** A new agent session can resume previous work with the right project, branch, files, decisions, tests, and unresolved work.

> **Layer 2 status:** Complete. New memories derive session/worktree/branch/project/global scopes, REST/MCP/CLI retrieval accepts current context, bounded context packs expose scope labels plus omitted-result recovery hints, session summaries expose deterministic next steps and unresolved work, and why-changed retrieval exposes evidence, relationships, stale signals, and contradictions. The local Layer 2 gate is the structural regression `scripts/measure_layer2.py` against the `cairn-golden/layer2-v0.1` corpus. It does not measure time or avoided investigation.

- [x] Capture repository, branch, worktree, commit, files, symbols, commands, tests, and services when available.
- [x] Keep session, branch/worktree, project, and user-global scopes distinct for local retrieval.
- [x] Assemble a bounded context pack with source IDs, scope labels, and omitted-result recovery hints.
- [x] Add “what happened last time?”, “why was this changed?”, and “what remains unresolved?” retrieval paths through deterministic handoff and evidence summaries.
- [x] Add deterministic code relationships inspired by Graphify where they improve context quality.
- [x] Add stale-memory and contradiction signals without silently selecting a winner.
- [x] Check the Layer 2 golden corpus with `scripts/measure_layer2.py`, a structural regression for why-changed evidence, branch isolation, stale and contradiction signals, and stable repeated investigation. It does not measure time or avoided investigation.

**Gate:** A new session receives relevant context without receiving unrelated branch-local hypotheses, can inspect why a change exists, and sees stale or contradictory evidence instead of an invisible winner. Verified by `python3 scripts/measure_layer2.py` as that structural regression, and by the full acceptance suite.

### Layer 3 — Personal Cross-Device Memory

> **Usable outcome:** A person's private memory follows them across trusted devices and agents.

> **Layer 3 status:** Complete for the personal cross-device boundary. File-backed and PostgreSQL modes expose the Cairn `/cairn-memory/` surface; PostgreSQL persists authenticated memory, context retrieval, corrections, export/import, lifecycle diagnostics, audit history, and sync diagnostics. The local outbox purges queued material after device revocation, while the cache and deletion policy is explicit and testable.

- [x] Add local person, agent, and device identity with same-person scoped access.
- [x] Add the first self-hosted account/session authentication adapter and account-authorized device registration.
- [x] Add a PostgreSQL Layer 3 adapter for account sessions, trusted devices, observations, sync events, and cursors.
- [x] Add server-authoritative PostgreSQL persistence for session summaries, explicit memories, forget tombstones, and keyword retrieval.
- [x] Add PostgreSQL parity for context packs, corrections, lifecycle diagnostics, export/import, and advanced retrieval signals.
- [x] Add device registration, rotation, revocation, and last-seen state.
- [x] Add a bounded local spool/outbox and authenticated replay for short outages.
- [x] Make replay idempotent and resumable after process, network, or server failure.
- [x] Define what clients may cache and how revoked or deleted material is purged.
- [x] Add sync lag, rejected events, recovery, and repair information to the viewer.

**Gate:** Memory captured on one trusted device is available exactly once on another without manual export/import, with cursor diagnostics, audit history, and revocation-safe outbox cleanup. Verified by the local and PostgreSQL Layer 3 smoke flows.

### Layer 4 — Project Cairn and Evidence-Backed Promotion

> **Usable outcome:** A repository has durable, branch-aware project knowledge created from verified personal work.

> **Layer 4 status:** The first project-promotion vertical slice and project
> entity inspection slice are complete:
> person-scoped project registration, explicit branch-to-project promotion,
> source evidence/provenance, repository matching, secret blocking, idempotent
> replay, project-scoped retrieval, branch/worktree registration, and project
> memory inspection are implemented in file-backed and PostgreSQL modes.
> Merge confirmation is implemented. Maintenance listing and linked correction
> history are implemented. The file-backed CLI and MCP can register a project,
> promote a memory, list project memories, and read freshness. Retiring project
> entities is not part of this layer.

- [x] Add project, repository, branch, worktree, and decision entities, with file, symbol, handoff, and runbook evidence carried in memory context.
- [x] Keep branch and worktree hypotheses scoped until merge, independent confirmation, or explicit promotion.
- [x] Add source-backed decision cards and evidence trails for files, commits, tests, and commands.
- [x] Add explicit promotion from personal or branch memory into project memory.
- [x] Mark project memories stale when later retrieval uses a different commit, and refresh them through explicit verification.
- [x] Add project search and context packs without requiring a team space.
- [x] Keep the viewer focused on inspection, exceptions, and correction rather than manual editing.

**Gate:** A developer can ask why a project decision exists and receive the decision, its evidence, its scope, and its freshness.

### Layer 5 — Collaborative Cairns

> **Usable outcome:** Authorized people and agents can share selected project memory without exposing private personal memory.

> **Layer 5 status:** An owner and one member can invite, explicitly share, read, and revoke one promoted project memory. The default space policy is manual, so promotion creates no share. The owner can list explainable share candidates, dismiss one without sharing it, or accept it through the existing share method. The only automatic policy is `auto_nonsensitive_project_memory`: promoting a project memory that is not secret and not sensitive shares it with current members and writes the share audit. Secrets, sensitive memories, and private observations stay unshared. Turning the policy off does not revoke existing shares. Owner and member can read a conflict list that keeps both contradictory project memories. Shared reads include the share reason and source memory id when those values exist. The project inspection page shows current decisions, handoffs, stale memories, conflicts, and revocations. The team space is project-scoped. The only roles are owner and member. The owner creates the team, names it, and adds or removes an existing project member. A member can list the team and its members. Removing a member removes that project membership and active shares. Delete retains the stored record: it hides the memory the same way forget does, keeps it in storage and export, and writes an audit event that the delete was retained. Offline purge, encryption, federation, and CRDTs remain unimplemented.

- [x] Add project and team spaces, memberships, roles, and permission checks.
- [x] Add explicit promote, share, revoke, forget, and delete operations with audit entries.
- [x] Add explainable share candidates and reviewable exceptions.
- [x] Allow automatic sharing only when an explicit space policy matches and privacy checks pass.
- [x] Preserve source actors and trails when memories are summarized or merged.
- [x] Represent contradictions and competing versions instead of silently choosing a winner.
- [x] Add collaboration views for handoffs, decisions, stale facts, conflicts, and revocations.

**Gate:** Two collaborators and their agents can use a shared project cairn while private observations remain private and every shared memory is traceable.

### Layer 6 — Advanced Intelligence and Operations

> **Usable outcome:** Cairn improves retrieval and memory maintenance at scale without weakening ownership or trust boundaries.

- [ ] Add semantic retrieval with a versioned embedding pipeline and exact-search evaluation.
- [ ] Add bounded graph-assisted retrieval across code, work, and memory graphs.
- [ ] Add durable background jobs with leases, retries, backoff, quarantine, and dead-letter inspection.
- [ ] Add automatic consolidation, staleness detection, contradiction detection, and promotion suggestions.
- [ ] Add local or server-side model options while keeping capture and basic recall available during provider failure.
- [ ] Expand the viewer into operational health, permissions, audit, and recovery surfaces.
- [ ] Add multimodal memory only with explicit privacy, retention, and storage policies.

**Gate:** Automatic processing improves recall and handoffs while provider failures or ambiguous classifications cannot broaden access.

### v1.0 — Trusted personal and project memory

> **Outcome:** Cairn is a stable, inspectable, portable memory source that works across a person's sessions, agents, projects, and trusted devices.

- [ ] Personal memory is automatic by default and remains person-owned.
- [ ] Session and branch context is scoped, evidence-backed, and retrievable.
- [ ] Personal memory synchronizes across trusted devices with recovery and revocation.
- [ ] Project memory supports explicit promotion, provenance, freshness, correction, and forget.
- [ ] REST, MCP, hook, sync, and export contracts are versioned and governed.
- [ ] The viewer explains what Cairn remembers, why it appeared, and how to correct or remove it.
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
- Personal memory autopilot precedes cross-device sync; cross-device sync precedes project promotion; project promotion precedes cross-person sharing; collaboration precedes federation.
- A phase may move only when evidence or a reviewed decision changes the priority.
- A feature that changes a public contract, storage model, trust boundary, or synchronization behavior needs an ADR before it enters an active phase.
