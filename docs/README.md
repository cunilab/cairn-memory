---
id: CM-DOCS-001
title: "Cairn Memory Documentation Index"
status: draft
---

# Cairn Memory Documentation Index

These documents describe Cairn Memory, an AI-managed, person-owned memory layer built on the AgentMemory foundation with Graphify-inspired code relationships, as implementation proceeds. They are intentionally small enough to review and change. Detailed feature behavior belongs in `docs/specs/` documents when a feature is ready to build.

`apps/web` is the first Next.js console. [Architecture](./architecture.md) names that console next to the Rust API.

| Document | ID | What it owns |
| --- | --- | --- |
| [Product](./product.md) | CM-PRODUCT-001 | AI-managed personal memory, ownership model, users, boundaries, and success signals |
| [Roadmap](./roadmap.md) | CM-ROADMAP-001 | Layered product phases, outcomes, and milestone gates |
| [Architecture](./architecture.md) | CM-ARCH-001 | Crate map, data ownership, interfaces, and the main flows of the 0.1.6 server |
| [Data flow guide](./data-flow.md) | CM-GUIDE-001 | Diagram-first explainer of capture, save, consolidation, recall, and projects as implemented in 0.1.6 |
| [Autopilot and sharing policy](./policy.md) | CM-POLICY-001 | Automatic capture, retention, privacy, ownership, correction, and sharing rules |
| [Phase 0 evidence](./evidence.md) | CM-EVIDENCE-001 | Recorded Phase 0 artifacts, source snapshot, checks, and limitations |
| [AgentMemory compatibility baseline](./contracts/agentmemory-baseline.md) | CM-CONTRACT-001 | Upstream REST, MCP, hook, CLI, environment, lifecycle, and portability contracts |
| [Layer 1 MCP compatibility contract](./contracts/mcp-layer1.md) | CM-CONTRACT-002 | Device-token network CLI and MCP stdio proxy, tool manifest, and limitations |
| [Layer 1 hook capture adapter](./contracts/hook-v1.md) | CM-CONTRACT-005 | `cairn hook` events, typed capture, injection budget, outbox and setup |
| [Layer 2 branch-aware continuity](./contracts/continuity-v1.md) | CM-CONTRACT-006 | Memory scopes, context-aware retrieval, and omitted-result recovery |
| [Layer 3 self-hosted account authentication](./contracts/auth-layer3.md) | CM-CONTRACT-008 | Account bootstrap, hashed sessions, logout, and device ownership |
| [Layer 4 project promotion](./contracts/project-layer4.md) | CM-CONTRACT-009 | Project registration and explicit evidence-backed promotion |
| [0.1.6 foundation specification](./specs/0.1.6-foundation.md) | CM-SPEC-004 | Schema, Rust model, module map, routes, test strategy and build order of the 0.1.6 rewrite |
| [Rust idioms and review rules](./RUST_IDIOMS.md) | CM-STD-002 | Rust quality rules for every agent and contributor |
| [Hook setup examples](./examples/README.md) | CM-GUIDE-002 | Claude Code hook wiring for `cairn hook` |
| [Phase 0 fixtures](./fixtures/phase-0/README.md) | CM-GUIDE-003 | Redacted contract fixtures and the golden corpus |
| [Release process](./release.md) | CM-REL-001 | Trunk-based flow, required checks, cutting a release, and how clients update |
| [Deployment layout](./deploy.md) | CM-DEPLOY-001 | Dokploy and Traefik layout for PostgreSQL, `cairn-server`, and the console, with the `/agent` API path |
| [Documentation standards](./standards.md) | CM-STD-001 | Document lifecycle, ownership, review, and evidence rules |
| [ADR 0001 — Compatibility-first rewrite](./adr/0001-compatibility-first-rewrite.md) | CM-ADR-0001 | Rationale for the migration strategy and major boundaries |
| [ADR 0002 — Collaborative sync model](./adr/0002-collaborative-sync-model.md) | CM-ADR-0002 | Identity, spaces, permissions, synchronization, and conflict boundaries |
| [ADR 0003 — Server-authoritative PostgreSQL](./adr/0003-server-authoritative-postgres.md) | CM-ADR-0003 | Server authority, database, embeddings, and search boundaries |
| [ADR 0004 — Trunk-based development and release](./adr/0004-trunk-based-development-and-release.md) | CM-ADR-0004 | Branch, worktree, CI, and GitHub release workflow |
| [ADR 0005 — Phase 0 product boundaries](./adr/0005-phase-0-product-boundaries.md) | CM-ADR-0005 | AI-managed personal memory, layered delivery, and unresolved decisions |
| [ADR 0006 — Layer 2 branch-aware retrieval](./adr/0006-layer2-branch-aware-retrieval.md) | CM-ADR-0006 | Why scope-aware retrieval comes before promotion and collaboration |
| [ADR 0007 — Layer 3 local trusted-device slice](./adr/0007-layer3-local-trusted-device-slice.md) | CM-ADR-0007 | Why local sync is proven before PostgreSQL and hosted authentication |
| [ADR 0008 — Self-hosted account authentication](./adr/0008-self-hosted-account-authentication.md) | CM-ADR-0008 | Why the first server uses self-hosted accounts and hashed bearer sessions |
| [ADR 0009 — Explicit project promotion](./adr/0009-explicit-project-promotion.md) | CM-ADR-0009 | Evidence-backed project registration and promotion |
| [ADR 0010 — Correction keeps linked history](./adr/0010-correction-keeps-linked-history.md) | CM-ADR-0010 | A correction keeps the old memory stored and linked; normal recall returns the new memory |
| [ADR 0011 — Explicit project memory share](./adr/0011-explicit-project-memory-share.md) | CM-ADR-0011 | Committed-file membership, one promoted memory grant, member read, and revoke |
| [ADR 0013 — Shared project decisions and personal memory scopes](./adr/0013-shared-project-decisions-and-personal-memory-scopes.md) | CM-ADR-0013 | Common project decisions, private scopes, publication, owner transfer and ID-based admission risk |
| [ADR 0014 — Memory scopes, distillation and hybrid recall](./adr/0014-memory-scopes-distillation-and-hybrid-recall.md) | CM-ADR-0014 | v0.1.4 scopes, distillation, hybrid recall, brief, agent guidance, retention score and feedback |
| [ADR 0012 — GitHub Releases and CLI self-update](./adr/0012-github-releases-and-self-update.md) | CM-ADR-0012 | Raw release assets, SHA256SUMS, `cairn update`, and the daily notice with their security limits |
| [Collaborative memory specification](./specs/collaborative-memory.md) | CM-SPEC-001 | Cross-device and cross-person memory behavior |
| [Memory context and promotion](./specs/memory-context-and-promotion.md) | CM-SPEC-002 | Scope escalation, branch/worktree context, and context packing |
| [Graph-assisted retrieval](./specs/graph-assisted-retrieval.md) | CM-SPEC-003 | Graph RAG, embeddings, hybrid retrieval, and code context |
| [Memory confidence and correction survey](./research/memory-confidence-and-correction.md) | CM-RESEARCH-001 | Source survey of confidence scales, relatedness cutoffs, and how a corrected record is kept or replaced |

## Archive

`archive/` holds records of work that no longer describes the code. Do not rely on them for current behavior.

- `archive/contracts/`: export v1, lifecycle v1, Layer 3 sync and the superseded Layer 5 share contract (the file store, import, sync pull and lifecycle fields were removed in 0.1.6).
- `archive/audits/`: the 2026-11-11 to 2026-11-13 UI and file-store audits (CM-AUDIT-001 to CM-AUDIT-015) with their screenshots.

## Planned later

These are intentionally not created yet:

- Additional feature specifications beyond the current `docs/specs/` set.
- `docs/security.md` — concrete risks and controls after the deployment boundary is defined.
- `docs/operations.md` — install, backup, recovery, and troubleshooting procedures after the runtime exists.
- `docs/launch.md` — release coordination when a release needs explicit readiness and rollback.

## Review order

1. Review [Product](./product.md) to confirm what is being built.
2. Review [Roadmap](./roadmap.md) to confirm the order and scope.
3. Review the [Autopilot and sharing policy](./policy.md) and [AgentMemory compatibility baseline](./contracts/agentmemory-baseline.md) before implementation.
4. Review [Architecture](./architecture.md), [ADR 0001](./adr/0001-compatibility-first-rewrite.md), [ADR 0002](./adr/0002-collaborative-sync-model.md), [ADR 0003](./adr/0003-server-authoritative-postgres.md), [ADR 0005](./adr/0005-phase-0-product-boundaries.md), [ADR 0006](./adr/0006-layer2-branch-aware-retrieval.md), [ADR 0007](./adr/0007-layer3-local-trusted-device-slice.md), and [ADR 0008](./adr/0008-self-hosted-account-authentication.md) to confirm the runtime and trust boundaries.
5. Review [ADR 0004](./adr/0004-trunk-based-development-and-release.md) before setting up concurrent implementation work.
6. Review [Memory context and promotion](./specs/memory-context-and-promotion.md) and [Graph-assisted retrieval](./specs/graph-assisted-retrieval.md) before implementing retrieval or promotion.
7. Use [Phase 0 evidence](./evidence.md) to check the artifact baseline, then move documents from `draft` to `proposed` only after explicit review.

One document should own each rule. Other documents link to that owner instead of copying it.
