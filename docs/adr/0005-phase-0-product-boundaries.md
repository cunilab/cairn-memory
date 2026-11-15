---
id: CM-ADR-0005
title: "ADR-0005: Establish the Phase 0 personal-memory boundaries"
status: draft
---

# ADR-0005: Establish the Phase 0 personal-memory boundaries

- **Date:** 2026-10-25
- **Milestone:** Phase 0 — product, contract, and memory-autopilot baseline
- **Related:** [Product](../product.md), [Policy](../policy.md), [Roadmap](../roadmap.md), [AgentMemory baseline](../contracts/agentmemory-baseline.md)

## Context

Cairn Memory is being built from AgentMemory's automatic capture and retrieval foundation, with Graphify as a reference for explainable code relationships. The product must provide value before it has cross-person collaboration, federation, advanced graph retrieval, or a complete operations console.

The central product promise is that a person should not need to curate memory manually. That promise is only trustworthy if automatic relevance decisions are separated from ownership and authorization decisions.

## Decisions

1. **Cairn is AI-managed and person-owned.** The AI manages relevance, compression, retention, retrieval, and suggestions. The person owns the memory and controls privacy, correction, deletion, export, and sharing.
2. **Personal memory is the first product layer.** A single person must get value from automatic capture, lifecycle management, and recall before collaboration is required.
3. **Every roadmap layer must be usable by itself.** Personal autopilot, branch continuity, cross-device memory, project promotion, and collaboration are separate product layers with gates.
4. **No manual save is required for normal use.** Explicit save remains a compatibility and correction operation, not the normal capture workflow.
5. **Relevance is not authorization.** A high-confidence or highly relevant memory cannot bypass privacy, scope, membership, revocation, or sharing rules.
6. **Personal-to-project promotion is explicit by default.** A branch merge, independent confirmation, or user action can create a promotion candidate; the first collaborative release requires approval unless a separate explicit policy enables automation.
7. **The first usable layer does not require a remote LLM or semantic embeddings.** Capture, privacy filtering, deterministic lifecycle behavior, keyword recall, provenance, export, and forget must have a keyless fallback.
8. **The viewer is an observer and exception surface.** It should explain memory decisions and provide correction, forget, export, promotion, sharing, and revocation controls without becoming a manual wiki editor.
9. **AgentMemory public behavior is the compatibility baseline.** REST, MCP, hook, CLI, environment, lifecycle, and export behavior are classified in the compatibility baseline before the Rust implementation changes them.

## Consequences

### Positive

- A user can get value from Cairn before identity, synchronization, and team permissions are complete.
- The product promise is simple: Cairn remembers the work automatically, while the person remains the owner.
- Privacy and sharing decisions have a separate policy boundary from ranking and retrieval.
- Capture stores the stricter of the supplied sensitivity label and the label detected from the original text, so a client cannot mark detected secret material as public. The patterns are in the lifecycle contract.
- The architecture can grow from one process and one store instead of requiring a distributed platform on day one.
- AgentMemory and Graphify remain useful foundations without forcing their internal implementation choices into Cairn.

### Costs and accepted risks

- Automatic retention and compression can be wrong; provenance, correction, and evidence are mandatory.
- The exact retention durations and deletion guarantees remain open until real fixtures and recovery tests exist.
- Some upstream AgentMemory features will be deferred or exposed only through compatibility adapters.
- A person-owned model may need stronger local privacy and encryption than a team-first design.

## Unresolved decisions

These are intentionally open and must be resolved in a later ADR or evidence update:

| Decision | Why it matters | Resolution gate |
| --- | --- | --- |
| First authentication model: self-hosted identity, external OIDC, or both | Resolved as self-hosted accounts with hashed bearer sessions in [ADR 0008](0008-self-hosted-account-authentication.md); OIDC remains later | Before Layer 3 leaves experimental status |
| Offline revocation and deletion guarantee | Determines whether cached material can be trusted after access changes | Before Layer 3 leaves experimental status |
| End-to-end encryption boundary | Determines which metadata the server can index and rank | Before sensitive shared memory |
| Default retention durations | Balances usefulness, privacy, storage, and recovery | After Layer 1 fixture evaluation |
| Supported self-hosted release boundary | Determines packaging, upgrades, backups, and operations scope | Before v1.0 |
| Exact AgentMemory export schema | Determines lossless migration and rollback | Before Layer 1 compatibility claim |

## Alternatives considered

### Collaboration-first platform

Rejected as the first product because it asks Cairn to solve identity, permissions, sharing, revocation, conflicts, deletion, and offline synchronization before proving that automatic personal memory is useful.

### Manual knowledge-base workflow

Rejected because it transfers the core memory burden back to the person and does not fulfill the promise of AI-managed memory.

### LLM-required memory

Rejected because provider cost, privacy, and outages must not prevent capture, deterministic lifecycle behavior, or basic recall.
