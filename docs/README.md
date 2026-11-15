---
id: CM-DOCS-001
title: "Cairn Memory Documentation Index"
status: draft
---

# Cairn Memory Documentation Index

These documents describe Cairn Memory, an AI-managed, person-owned memory layer built on the AgentMemory foundation with Graphify-inspired code relationships, as implementation proceeds. They are intentionally small enough to review and change. Detailed feature behavior belongs in `docs/specs/` documents when a feature is ready to build.

`apps/web` is the first Next.js console. [Architecture](./architecture.md) names that console next to the Rust API and the embedded viewer.

| Document | ID | What it owns |
| --- | --- | --- |
| [Product](./product.md) | CM-PRODUCT-001 | AI-managed personal memory, ownership model, users, boundaries, and success signals |
| [Roadmap](./roadmap.md) | CM-ROADMAP-001 | Layered product phases, outcomes, and milestone gates |
| [Architecture](./architecture.md) | CM-ARCH-001 | Edge, sync, system structure, interfaces, data ownership, and flows |
| [Data flow guide](./data-flow.md) | CM-GUIDE-001 | Diagram-first explainer of capture, memory lifecycle, recall, projects, sharing, authentication, sync, use cases, and tools as implemented today |
| [Autopilot and sharing policy](./policy.md) | CM-POLICY-001 | Automatic capture, retention, privacy, ownership, correction, and sharing rules |
| [Phase 0 evidence](./evidence.md) | CM-EVIDENCE-001 | Recorded Phase 0 artifacts, source snapshot, checks, and limitations |
| [AgentMemory compatibility baseline](./contracts/agentmemory-baseline.md) | CM-CONTRACT-001 | Upstream REST, MCP, hook, CLI, environment, lifecycle, and portability contracts |
| [Layer 1 MCP compatibility contract](./contracts/mcp-layer1.md) | CM-CONTRACT-002 | Device-token network CLI and MCP stdio proxy, tool manifest, and limitations |
| [Cairn export format v1](./contracts/export-v1.md) | CM-CONTRACT-003 | Versioned portable state, import merge rules, and tombstones |
| [Layer 1 lifecycle classification](./contracts/lifecycle-v1.md) | CM-CONTRACT-004 | Importance, sensitivity, confidence, freshness, retention, and expiration |
| [Layer 1 hook capture adapter](./contracts/hook-v1.md) | CM-CONTRACT-005 | Portable automatic hook ingestion and normalized capture fields |
| [Layer 2 branch-aware continuity](./contracts/continuity-v1.md) | CM-CONTRACT-006 | Memory scopes, context-aware retrieval, and omitted-result recovery |
| [Layer 3 trusted device sync](./contracts/sync-layer3.md) | CM-CONTRACT-007 | Device identity, authenticated replay, cursors, rotation, revocation, and outbox |
| [Layer 3 self-hosted account authentication](./contracts/auth-layer3.md) | CM-CONTRACT-008 | Account bootstrap, hashed sessions, logout, and device ownership |
| [Layer 4 project promotion](./contracts/project-layer4.md) | CM-CONTRACT-009 | Project registration and explicit evidence-backed promotion |
| [Layer 5 explicit project memory share](./contracts/share-layer5.md) | CM-CONTRACT-010 | Owner invite, project team, explicit share, member read, revoke, and retained delete |
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
| [ADR 0011 — Explicit project memory share](./adr/0011-explicit-project-memory-share.md) | CM-ADR-0011 | Owner invite, one promoted memory grant, member read, and revoke |
| [ADR 0012 — GitHub Releases and CLI self-update](./adr/0012-github-releases-and-self-update.md) | CM-ADR-0012 | Raw release assets, SHA256SUMS, `cairn update`, and the daily notice with their security limits |
| [Collaborative memory specification](./specs/collaborative-memory.md) | CM-SPEC-001 | Cross-device and cross-person memory behavior |
| [Memory context and promotion](./specs/memory-context-and-promotion.md) | CM-SPEC-002 | Scope escalation, branch/worktree context, and context packing |
| [Graph-assisted retrieval](./specs/graph-assisted-retrieval.md) | CM-SPEC-003 | Graph RAG, embeddings, hybrid retrieval, and code context |
| [Memory confidence and correction survey](./research/memory-confidence-and-correction.md) | CM-RESEARCH-001 | Source survey of confidence scales, relatedness cutoffs, and how a corrected record is kept or replaced |
| [Desktop UI baseline — 2026-11-11](./audits/2026-11-11-desktop-ui-baseline.md) | CM-AUDIT-001 | Desktop Playwright observations for login, projects, and project detail |
| [shadcn/ui and Next.js research — 2026-11-11](./audits/2026-11-11-shadcn-next.md) | CM-AUDIT-002 | Installed UI stack compared with current shadcn/ui and Next.js docs |
| [Mobile and accessibility — 2026-11-11](./audits/2026-11-11-mobile-a11y.md) | CM-AUDIT-003 | Phone-width layout and focused accessibility checks |
| [Sign-in flow task 1 — 2026-11-12](./audits/2026-11-12-signin-task.md) | CM-AUDIT-004 | Auth proxy, sign-in, and session redirect evidence for the first UI fix |
| [Projects list task 2 — 2026-11-12](./audits/2026-11-12-projects-list.md) | CM-AUDIT-005 | Projects list card target, empty state, and loading evidence |
| [Project detail display task 3 — 2026-11-12](./audits/2026-11-12-project-detail.md) | CM-AUDIT-006 | Project detail empty team, missing project, and wrapping evidence |
| [Delete and revoke confirmation task 4 — 2026-11-12](./audits/2026-11-12-actions.md) | CM-AUDIT-007 | Confirmation dialogs for retained delete and share revoke |
| [Memory capture and persistence — 2026-11-12](./audits/2026-11-12-memory-capture.md) | CM-AUDIT-008 | Capture admission, redaction, event identity, and file/PostgreSQL durability |
| [Account search limit — 2026-11-13](./audits/2026-11-13-account-search-limit.md) | CM-AUDIT-009 | File account search, resume, and unresolved keep that person's rows and contradiction ids; other retrieval residuals stay open |
| [Recall filter and contradiction targets — 2026-11-13](./audits/2026-11-13-recall-filter-eligibility.md) | CM-AUDIT-010 | Global filter inheritance is contractual; a contradiction target must itself be eligible for recall |
| [PostgreSQL search cap — 2026-11-13](./audits/2026-11-13-postgres-search-cap.md) | CM-AUDIT-011 | The 512-row search bound is undocumented; context and why-changed report from the capped list |
| [File store supersedes ownership — 2026-11-13](./audits/2026-11-13-file-supersedes-ownership.md) | CM-AUDIT-012 | A foreign `supersedes` link no longer hides another account's memory in the file store (fixed, review accepted); foreign `forgottenIds` on import is open and a project-import gap is unverified |
| [File import tombstone ownership — 2026-11-13](./audits/2026-11-13-file-import-tombstone-ownership.md) | CM-AUDIT-013 | A file account import could tombstone ids it does not own and hide another account's memory; finding 1 fixed with option 1 (owned or same-bundle ids only) and independently accepted; finding 2 (an account can forget an unowned legacy row) and finding 3 (tombstoning another account's ids through the `source_event_ids` of the caller's own row) were reproduced and fixed with strict ownership for account forget and source cascades and independently accepted for the file store; the project-import gap is CM-AUDIT-014 |
| [File import project_id ownership — 2026-11-13](./audits/2026-11-13-file-import-project-id.md) | CM-AUDIT-014 | A file account import can add a row with another account's `project_id` to that account's owner project view and hide its project memory there (reproduced; fixed with owner-only project-linked imports and owner views that skip other accounts' rows; finding 1 independently accepted for the file store; the `project_maintenance` caller check, PostgreSQL and old-data repair stay open) |
| [Project registry access — 2026-11-13](./audits/2026-11-13-project-registry-access.md) | CM-AUDIT-015 | The file-store project registry (branches, worktrees, entities, freshness, merge, close) was open to any account that knew the project id and a merge could copy another account's memory; fixed with owner-only access and independently accepted for the file store, nine registry operations (maintenance accepted separately); PostgreSQL, old aliased branches, the first-registrar claim and lenient helpers elsewhere stay open |

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
