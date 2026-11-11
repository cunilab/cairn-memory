---
id: CM-ADR-0001
title: "ADR-0001: Use a compatibility-first incremental rewrite"
status: draft
---

# ADR-0001: Use a compatibility-first incremental rewrite

- **Date:** 2026-10-25
- **Milestone:** Phase 0 — product and contract baseline
- **Related:** [Product](../product.md), [Roadmap](../roadmap.md), [Architecture](../architecture.md)

## Context

The target product, Cairn Memory, is a Rust backend with a Next.js and shadcn/ui frontend. It builds on the upstream AgentMemory product, which already has important external behavior: agent hooks, REST endpoints, MCP tools, local state, export/import, capture durability, search ranking, privacy controls, and background processing.

The current runtime is organized around a TypeScript worker and `iii-engine`. A direct file-by-file port would reproduce implementation details without clarifying the product boundaries. A big-bang replacement would also make it difficult to tell whether a regression came from storage, capture, ranking, provider behavior, or transport compatibility.

The rewrite needs to take time and proceed step by step. Cairn Memory first adds AI-managed personal continuity and branch-aware project context, then cross-device synchronization and cross-person collaboration. The migration strategy must preserve the foundation while adding identity, spaces, permissions, and sync without making private memory public by accident.

## Decision

Adopt a **compatibility-first strangler migration**:

1. Treat REST, MCP, hook payloads, CLI behavior, environment configuration, and versioned export/import as explicit contracts.
2. Define the product and architecture before implementing the backend.
3. Build a Rust application service layer that is independent of HTTP, MCP, jobs, and the web console.
4. Use a server-authoritative PostgreSQL source of truth, with client edge spools/caches limited to outage tolerance and search indexes, embeddings, graph projections, and event counts treated as derived state.
5. Implement the core personal memory loop first: session lifecycle, observation capture, privacy filtering, automatic classification and retention, deduplication, keyword recall, durable memories, evidence-backed context assembly, export/import, health, and status.
6. Keep existing hooks working against the Rust REST surface during the first compatibility milestones.
7. Implement essential MCP tools with the same names and semantics before porting the complete tool surface.
8. Build the Next.js/shadcn console against the documented REST and sync contracts rather than embedding product logic in the frontend.
9. Run the TypeScript and Rust runtimes on separate ports or against separate stores during comparison. Do not let two runtimes mutate the same canonical store without an explicit dual-write design.
10. Require golden-corpus search comparisons, restart/recovery checks, contract tests, sync tests, permission tests, and migration tests before making the Rust runtime the default.

## Consequences

### Positive

- The rewrite can deliver useful milestones without waiting for every advanced feature.
- Existing agent integrations remain usable while the backend changes internally.
- Regressions can be localized by comparing one contract or service boundary at a time.
- PostgreSQL keeps the server source of truth transactional and queryable while the optional edge spool handles short outages.
- Derived indexes can evolve without turning search-index compatibility into a data-loss risk.
- The web console and agent tools share one service layer and one source of truth.

### Costs and accepted risks

- The project temporarily carries two implementations and two sets of compatibility tests.
- Some advanced behavior will remain unavailable until later phases.
- Preserving old contracts may retain awkward names, payloads, or error shapes.
- Search parity may require more than matching API shapes; tokenization, ranking, graph expansion, authorization filtering, and provider timing must be measured.
- Collaboration adds identity, permission, revocation, conflict, deletion, and offline-sync risks that do not exist in a single-device memory store.
- The migration needs explicit data fixtures and recovery tests before claims of parity are credible.

## Alternatives considered

### Big-bang rewrite

Rejected because it combines storage migration, protocol changes, identity, sharing, synchronization, search, provider changes, and frontend work into one difficult-to-debug cutover.

### Keep `iii-engine` and only rewrite the frontend

Rejected as the long-term direction because it does not address the requested Rust backend or simplify the runtime boundary. It may still be useful as a temporary reference implementation during compatibility testing.

### Introduce an external database and vector service first

Rejected for the first milestone because it would move the product away from its local-first edge model before core memory and collaboration behavior is validated in Rust.

### Rewrite the frontend first

Rejected as the first workstream because the frontend depends on stable domain and API contracts. Documentation and backend boundaries should be reviewed before building a dashboard around them.
