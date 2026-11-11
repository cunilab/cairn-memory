---
id: CM-EVIDENCE-001
title: "Phase 0 Evidence — Product and Contract Baseline"
status: draft
---

# Phase 0 Evidence — Product and Contract Baseline

| Field | Value |
| --- | --- |
| Scope | Phase 0 product definition, upstream contract inventory, policy, fixtures, and golden corpus |
| Date | 2026-10-25 |
| Implementation state | Local Layer 1 personal-memory vertical slice is complete: `cairn-core`, REST, MCP, CLI, hook adapter, export/import, and local viewer exist; the Next.js product console is later work |

## Result

Phase 0 artifacts now exist in the workspace, and Layer 1 now exercises capture, redaction, lifecycle classification, lesson and correction workflows, deterministic session compression, keyword recall, bounded context packing, forget, retention sweep, export/import, restart persistence, hook ingestion, CLI, MCP, REST, and the local viewer. A reviewer can inspect the product promise, the AI-managed memory policy, the upstream compatibility surface, representative safety fixtures, and a versioned retrieval corpus.

This is **not** evidence that the complete Cairn runtime is implemented. It is evidence that the first implementation boundary is explicit and that the smallest capture-to-recall slice has executable tests.

## Phase 0 checklist

| Requirement | Evidence | Status |
| --- | --- | --- |
| Inventory upstream REST, MCP, hooks, CLI, environment, export, and lifecycle behavior | [AgentMemory compatibility baseline](contracts/agentmemory-baseline.md) | Recorded with source references and explicit verification gaps |
| Classify interfaces | Compatibility categories and inventory table in [CM-CONTRACT-001](contracts/agentmemory-baseline.md) | Complete for the initial baseline; full tool/schema diff remains a Layer 1 gate |
| Define vocabulary | [Product model](product.md#product-model), [Policy](policy.md) | Complete for Phase 0 |
| Define automatic capture, retention, correction, privacy, and sharing behavior | [Cairn Memory Autopilot and Sharing Policy](policy.md) | Complete as a proposed product policy |
| Build representative fixtures | [`fixtures/phase-0/`](../fixtures/phase-0/) | Complete as redacted contract fixtures |
| Create a versioned golden corpus | [`fixtures/phase-0/golden-corpus/v0.1/cases.json`](../fixtures/phase-0/golden-corpus/v0.1/cases.json) | Complete as a baseline; no score is claimed yet |
| Record unresolved decisions in ADRs | [ADR 0005](adr/0005-phase-0-product-boundaries.md) and [collaborative memory open questions](specs/collaborative-memory.md#open-questions) | Complete; unresolved items remain intentionally open |

## Source snapshot

The upstream inventory was reviewed from these public files on October 25, 2026:

- [AgentMemory README](https://github.com/rohitg00/agentmemory/blob/main/README.md)
- [AgentMemory package manifest](https://github.com/rohitg00/agentmemory/blob/main/package.json)
- [AgentMemory environment template](https://github.com/rohitg00/agentmemory/blob/main/.env.example)
- [AgentMemory hook manifest](https://github.com/rohitg00/agentmemory/blob/main/plugin/hooks.json)
- [AgentMemory installation and verification runbook](https://github.com/rohitg00/agentmemory/blob/main/INSTALL_FOR_AGENTS.md)

The observed upstream package version is `0.9.30`; the observed runtime baseline is Node.js 20+ with iii-engine `0.22.1`. The source is the upstream `main` branch rather than a pinned commit, so the contract baseline must be pinned before compatibility implementation begins.

## Local validation

The following checks were run from the workspace root after creating the Phase 0 artifacts:

```text
python3 scripts/validate_phase0.py
cargo test
./scripts/smoke_layer1.sh
./scripts/smoke_rest.sh
./scripts/smoke_mcp.sh
./scripts/smoke_export.sh
./scripts/smoke_hook.sh
./scripts/smoke_viewer.sh
./scripts/smoke_full_flow.sh
python3 scripts/measure_layer2.py
./scripts/smoke_layer3.sh
```

Layer 1 completion checks are now present. Layer 2 is implemented and verified:

- branch/worktree/project/global scope derivation;
- context-aware REST, MCP, and CLI retrieval;
- branch isolation and omitted-result recovery hints;
- structured files, symbols, commands, tests, and services context;
- deterministic session handoff extraction for next steps and unresolved work;
- why-changed retrieval with decision reasons and source evidence;
- bounded file/symbol/test/service relationships;
- stale-commit and contradiction signals;
- Layer 2 golden-corpus measurement for first-informed-action and repeated investigation;
- real-process acceptance coverage for scoped retrieval.

## Limitations

- The Rust core, REST transport, and MCP stdio transport described by this Phase 0 evidence are local adapters; the Layer 3 PostgreSQL boundary is tracked separately in the Layer 3 contracts and migration evidence.
- The local Layer 1 retention policy uses a 30-day default for temporary raw observations; product-wide retention durations remain policy-configurable later.
- Full upstream AgentMemory registry parity and the Next.js product console remain outside this local Layer 1 completion gate; PostgreSQL-derived Layer 2 retrieval is now covered by the Layer 3 checks.
- `scripts/smoke_full_flow.sh` is the end-to-end acceptance check: it uses actual Cairn server processes, the real hook adapter, temporary stores, HTTP requests, restart recovery, correction, forget, and restore verification.
- The same acceptance flow now creates project, worktree, and global cache memories and verifies that `main` context excludes the worktree hypothesis while the matching worktree can retrieve it.
- `scripts/measure_layer2.py` passed the four-case `cairn-golden/layer2-v0.1` corpus: why-changed evidence/relationships, branch isolation, stale signal, contradiction signal, and stable repeated investigation.
- `scripts/smoke_layer3.sh` passed the trusted-device slice: registration, authenticated outbox replay, secret redaction, duplicate push, cross-device pull, cursor resume, last-seen state, token rotation, revocation, and purge of queued material after a revoked credential.
- `crates/storage/tests/postgres.rs` records the PostgreSQL account/device/sync, session-summary, explicit-memory, search, and forget flow; it passed against PostgreSQL 16 running in Colima with `CAIRN_TEST_DATABASE_URL`.
- `scripts/smoke_postgres.sh` passed the PostgreSQL HTTP boundary against the same Colima-backed database, including account bootstrap, device lifecycle, sync replay, cursor pull, redaction, revocation, project registration, and explicit promotion.
- The PostgreSQL integration and HTTP smoke flows also cover context packing, why-changed signals, correction supersession, idempotent export/import, lifecycle diagnostics, sync lag diagnostics, and metadata-only audit history.
- `scripts/smoke_layer3.sh` verifies revoked-device outbox purge, while `GET /cairn-memory/sync/diagnose` and the viewer health panel expose cursor lag and recovery information.
- Authentication attempts are covered by a server test for the per-identifier rate limit; PostgreSQL audit history is metadata-only and excludes bearer tokens, passwords, and memory content.
- The first Layer 4 vertical slice is covered by the file-backed server test and PostgreSQL integration/smoke paths: project registration, explicit promotion, source evidence, cleared branch/worktree context, idempotent replay, repository matching, and secret-promotion rejection.
- Project freshness is covered by stale retrieval against a different commit and explicit verification through `/cairn-memory/projects/verify`; verification preserves provenance while restoring freshness to 100.
- Project entity inspection is covered by branch/worktree registration and project-memory listing in the local and PostgreSQL tests/smoke flow; registration is person-scoped and idempotent.
- Merge confirmation and branch closure are covered by local and PostgreSQL tests: merge evidence is branch-matched, project freshness is reset to the merge commit, retries reuse the same decision, attached worktrees are removed, and closed branches cannot be merged.
- The viewer is a dependency-free local HTML surface served by `cairn-server`; the planned Next.js product console remains a later UI layer.
- The MCP adapter covers the Layer 1 compatibility set and direct capture aliases, not the upstream full registry or modern protocol-era discovery.
- The upstream full export schema and complete MCP argument/result registry still need source-level extraction; Cairn's local v1 export is defined in [CM-CONTRACT-003](contracts/export-v1.md).
- The golden corpus defines expected behavior but has not been scored against a running implementation.
- Retention durations and the future encrypted-cache implementation remain policy-configurable follow-up work; the Layer 3 policy forbids unencrypted response caches and defines tombstone/revocation purge behavior. The first authentication model is resolved by ADR 0008.
- Stale/superseded maintenance and cross-person project membership remain Layer 4/5 follow-up work; the project promotion and merge boundary is defined in [CM-CONTRACT-009](contracts/project-layer4.md) and [ADR 0009](adr/0009-explicit-project-promotion.md).
