---
id: CM-POLICY-001
title: "Cairn Memory Autopilot and Sharing Policy"
status: draft
---

# Cairn Memory Autopilot and Sharing Policy

Current project collaboration is defined by [ADR 0013](adr/0013-shared-project-decisions-and-personal-memory-scopes.md).
Its all-member decisions, explicit publication, personal scopes and online retrieval
supersede the selective-sharing, team-space and project-delivery proposals below.
The current API contract is [project-layer4](contracts/project-layer4.md).

This policy defines the proposed Phase 0 behavior in plain language. It is the product boundary for automatic memory. The AI may manage relevance and lifecycle, but it does not get to redefine ownership or permissions.

## The promise

When a person connects Cairn to an agent, Cairn should remember useful work automatically. The person should not need to decide what to save, where to file it, which tag to use, or how to summarize a session.

The person owns the resulting memory. Cairn must make its decisions inspectable and give the person a clear way to correct, forget, export, or restrict memory.

## Three control boundaries

| Boundary | Cairn may do automatically | Cairn may not do automatically |
| --- | --- | --- |
| Relevance | Decide that an observation is useful, redundant, stale, temporary, or worth compressing | Treat relevance as permission to share or expose the item |
| Trust | Apply privacy filters, scope rules, retention policy, and authorization checks | Broaden visibility because a result has a high score or a plausible summary |
| Ownership | Suggest corrections, promotion, or forget actions and keep source evidence | Permanently delete, share private memory, or override a person's correction without an explicit authorized rule |

## Capture and filtering

1. Capture is automatic for an enabled integration and is bounded by payload, output, retry, and spool limits.
2. Secrets, credentials, bearer tokens, private keys, and other configured sensitive material are redacted before provider calls, synchronization, indexing for broader scopes, or sharing. For capture, explicit remember, and correction, the stored sensitivity label is at least the label detected from the original content and from the human-readable context lists and concepts named in [lifecycle classification](archive/contracts/lifecycle-v1.md). A caller may set a stricter label and cannot set a weaker one. Import redacts those same strings and keeps the bundle's sensitivity label. A kept public label does not mean the original metadata was free of secrets. Already stored rows are not rewritten.
3. A capture event records its source, session, actor, device when available, timestamp, and idempotency identity.
4. A provider failure does not discard accepted capture. The system keeps the event for later processing or makes it available through deterministic fallback behavior.
5. Capture does not imply prompt injection. Background capture and context injection are separate controls.

## Automatic memory lifecycle

The memory autopilot may:

- classify importance, sensitivity, confidence, freshness, and utility;
- deduplicate repeated observations;
- compress noisy observations into a source-backed summary;
- keep high-value durable memory;
- let low-value temporary material expire or remain outside normal recall;
- detect contradictions, stale facts, and likely corrections;
- retain evidence and the explicit scope selected by the agent.

Every durable memory must retain source IDs and enough evidence to answer:

- Why does Cairn remember this?
- When was it last verified?
- Which session, file, symbol, command, test, or commit supports it?
- What later event could make it stale or wrong?
- Which scope and actor currently own it?

## Default scopes

The current boundary is [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md), items 0 and 7; [the scope contract](contracts/project-layer4.md#memory-scopes-and-record) owns exact wire fields and permissions.

| Scope | Visibility and control |
| --- | --- |
| Project | All members read notes, raw captures and summaries from initialized repositories; author or current owner edits |
| Personal behaviour | Private to the person, including preferences and habits; no personal-per-repository namespace |
| Global | All server users read; author or administrator edits and deletes |

Agents explicitly choose scope on save. Initialized-repository capture directly creates project memory; there is no promotion, preview or approval step. Redaction runs before storage. Project members can read raw capture content after redaction, including sensitive/secret classifications; redaction is best effort.

Summaries mixing personal captures or different projects stay personal rather than broadening the source visibility. Import restores personal records, never project/global publication. A personal export excludes project and global memory records and is not a server backup.

## Correction, forget, and deletion

- **Correct** creates a new auditable state transition that supersedes the incorrect derived memory while preserving the source trail.
- **Forget** removes an item from normal retrieval and records a tombstone or equivalent suppression state.
- **Delete** uses the same tombstone as forget. The stored record and export copy stay. The audit action `memory_delete_retained` records that the delete was retained. Delete does not purge other devices. Offline purge remains open.
- **Revoke** stops future access for the affected device, actor, or membership and records whether cached clients acknowledged the purge instruction.
- A correction or forget action by the owner must outrank an automated relevance suggestion.

## Viewer behavior

The viewer is not a manual memory-management application. It should show:

- what Cairn remembers;
- why the item was retained or retrieved;
- the source trail and scope;
- uncertain, stale, conflicting, sensitive, or failed items;
- sync and provider health;
- the small set of actions that require a person: correct, forget, export, or revoke.

The normal path is automatic. The UI is for inspection and exceptions.

## Phase 0 unresolved decisions

The following remain intentionally open and must not be silently decided during implementation:

1. Whether the first server uses self-hosted identity, external OIDC, or both.
2. What deletion guarantee applies to a device that is offline when access is revoked.
3. Which payloads are end-to-end encrypted and which metadata must remain queryable by the sync plane.
4. Whether the first deployment supports self-hosted server mode as a supported release target or only as an engineering path.
5. The exact default retention durations for raw observations, session summaries, and durable memories. The local Layer 1 prototype uses a 30-day expiry for temporary raw observations, no expiry for durable summaries and lessons, and explicit user-provided expiry when supplied.

These decisions are recorded in [ADR 0005](adr/0005-phase-0-product-boundaries.md) and the collaborative memory specification.
