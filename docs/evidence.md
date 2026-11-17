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
| Implementation state | Local Layer 1 personal-memory vertical slice is complete: `cairn-core`, REST, hook adapter, export/import, and local viewer exist; the CLI and MCP are device-token network clients (`crates/client`); `apps/web` is the first Next.js console |

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
- Layer 2 golden-corpus structural regression (`scripts/measure_layer2.py`), which does not measure time or avoided investigation;
- real-process acceptance coverage for scoped retrieval.

## Limitations

- The Rust core and REST transport described by this Phase 0 evidence are local adapters, and the MCP stdio transport is now the `cairn mcp` REST client; the Layer 3 PostgreSQL boundary is tracked separately in the Layer 3 contracts and migration evidence.
- The local Layer 1 retention policy uses a 30-day default for temporary raw observations; product-wide retention durations remain policy-configurable later.
- Full upstream AgentMemory registry parity and the Next.js product console remain outside this local Layer 1 completion gate; PostgreSQL-derived Layer 2 retrieval is now covered by the Layer 3 checks.
- `scripts/smoke_full_flow.sh` is the end-to-end acceptance check: it uses actual Cairn server processes, the real hook adapter, temporary stores, HTTP requests, restart recovery, correction, forget, and restore verification.
- The same acceptance flow now creates project, worktree, and global cache memories and verifies that `main` context excludes the worktree hypothesis while the matching worktree can retrieve it.
- `scripts/measure_layer2.py` is a structural regression. It passed the four-case `cairn-golden/layer2-v0.1` corpus: why-changed evidence/relationships, branch isolation, stale signal, contradiction signal, and stable repeated investigation. It does not measure time or avoided investigation.
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
- The embedded viewer is a dependency-free local HTML surface served by `cairn-server`; `apps/web` is the first Next.js console and the viewer remains.
- The CLI and MCP no longer open a store file or take an account token. `cargo test -p cairn-client` passed on 2026-11-15 (15 unit tests and 5 integration tests in `crates/client/tests/agent_flow.rs`, which run the real `cairn` binary and `cairn mcp` against the file-store router; the PostgreSQL case is ignored without `CAIRN_TEST_DATABASE_URL`). The credential rules are in [CM-CONTRACT-008](contracts/auth-layer3.md) and the tool table in [CM-CONTRACT-002](contracts/mcp-layer1.md).
- The MCP adapter covers the Layer 1 compatibility set and direct capture aliases, not the upstream full registry or modern protocol-era discovery.
- The upstream full export schema and complete MCP argument/result registry still need source-level extraction; Cairn's local v1 export is defined in [CM-CONTRACT-003](contracts/export-v1.md).
- The golden corpus defines expected behavior but has not been scored against a running implementation.
- Retention durations and the future encrypted-cache implementation remain policy-configurable follow-up work; the Layer 3 policy forbids unencrypted response caches and defines tombstone/revocation purge behavior. The first authentication model is resolved by ADR 0008.
- Project maintenance and cross-person membership are implemented: the maintenance report lists stale and superseded project memories, and a teammate joins a project through the committed `.cairn/project.json`, an owner can share a promoted memory with that member, and keep a project team. The promotion and merge boundary is defined in [CM-CONTRACT-009](contracts/project-layer4.md) and [ADR 0009](adr/0009-explicit-project-promotion.md). Offline purge of a device that never syncs again, encryption, and federation remain unimplemented.

## v0.1.4 stage 1 — memory scopes and record shape

Verified 2026-11-17 on branch `v0.1.4`, limited to [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md) items 0 and 7. No live Cairn server was contacted. Every verification shell cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN` and `CAIRN_OUTBOX`; server smokes used disposable loopback file stores, and PostgreSQL used a disposable local PostgreSQL 17 container and fresh database per test.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check`, `cargo check --workspace` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --workspace` | Passed; ignored database tests executed separately |
| `scripts/ci_postgres_tests.sh` | 29 tests passed, zero failed; scratch databases dropped |
| `python3 scripts/smoke_hook_auth.py` | 25 tests passed |
| `python3 scripts/smoke_hook_offline.py` | 23 tests passed |
| `python3 scripts/smoke_outbox.py` | 32 tests passed |
| `scripts/smoke_layer4_cli.sh`, `scripts/smoke_layer5_cli.sh`, `scripts/smoke_mcp.sh` | Passed |
| Web `npm run typecheck`, `npm run lint`, `npm run build` | Passed |
| `scripts/smoke_web.sh` | Passed, including the Memories page route |
| `python3 scripts/validate_phase0.py` | Passed |

New assertions cover project-member access to captures and summaries, private personal behaviour, server-wide global reads, author/admin global mutation, required save fields, all six typed field groups, title weighting, full list/get records, removed promotion/private-repository routes and tools, and private fallback for mixed-source summaries. Title-weight tests failed first on both stores, then passed after the lexical-index change; required-save tests were added before the save entrypoints were implemented. Obsolete publication-copy/preview tests were removed with those entrypoints; retained capture, authentication, ownership, sync, lifecycle, import, branch and rename checks remain in the suites.

This is a pre-release schema change, not an in-place upgrade: start a fresh PostgreSQL database and file store. The edited development base migration adds `record`, personal defaults and a weighted lexical index. Applying only the column change to an old database is unsafe because its old private `global` rows require reclassification. No legacy repair or live migration was executed. Browser visual inspection and later ADR 0014 stages were not run or implemented.

## v0.1.4 stage 2 — capture hygiene and session distillation

Verified 2026-11-17 on `v0.1.4`, limited to ADR 0014 items 1 and 5 and the
owner-confirmed shared-note forgetting boundary. Every verification shell
cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN` and `CAIRN_OUTBOX`. No live Cairn API
was contacted; server smokes used disposable loopback file stores, and the
database suite used a disposable PostgreSQL 17 container on Colima with a
fresh database per test.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check`, `cargo check --workspace` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --workspace` | 179 passed; 30 database tests ignored here and executed below |
| `scripts/ci_postgres_tests.sh` | 30 passed, zero failed; scratch databases dropped |
| `python3 scripts/smoke_hook_auth.py` | 29 passed |
| `python3 scripts/smoke_hook_offline.py` | 23 passed |
| `python3 scripts/smoke_outbox.py` | 32 passed |
| `scripts/smoke_full_flow.sh`, `scripts/smoke_layer3.sh` | Passed |
| `scripts/smoke_mcp.sh`, `scripts/smoke_hook.sh` | Passed |
| `python3 scripts/validate_phase0.py` | Passed |

Hook regressions failed before the hygiene implementation, then passed for
assistant opt-in, marker lookup/skip without network or queue writes, and
redaction before both truncation boundaries. Shared HTTP scenarios were
written before adapter changes and run on both stores: session note/handoff,
all five explicit note prefixes, source evidence, structured fields without
inference, project-member reads, private fallback, unchanged repeated end,
late captures, correction and forgetting. Project-note forgetting preserves
shared source captures and siblings; explicit source-event forgetting hides
every derivation. The file-store parity regression failed before that fix.
Core tests also cover bounded extraction, metadata termination, case-insensitive
prefixes and ID collisions with legal session names; the ID regression failed
before separate handoff/typed namespaces were introduced.

The full-flow fixtures now use the stage-1 required save fields and scopes:
initialized project records, private branch work captures and personal
preferences. They retain restart, correction, forgetting, export/import,
context omission and branch-isolation checks, including separate handoffs.
MCP and hook assertions likewise account for the additional record.

No web files changed, so web checks and browser inspection were not run.
Independent worker review was not run in this single-worker dispatch. No LLM,
new dependency, hybrid recall, retention scoring, feedback or instruction
injection was added. The release setup smoke still emits its existing 404
warning for unreleased 0.1.4 hook assets; its local MCP assertions pass.

## v0.1.4 stage 3 — hybrid recall

Verified 2026-11-17 on `v0.1.4`, limited to ADR 0014 item 2 and the necessary
runtime/release changes. Every shell cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`
and `CAIRN_OUTBOX`. No live Cairn API was contacted. Database checks used only
the disposable local `pgvector/pgvector:pg17` container and fresh databases;
server smokes used temporary loopback stores.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check`, `cargo check --workspace --locked` | Passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo test --workspace --locked` | 184 passed; 33 database/model tests ignored here and executed separately |
| `scripts/ci_postgres_tests.sh` | 33 passed, zero failed; includes real MiniLM tests on both stores |
| Final compiled PostgreSQL hybrid test | Passed again after the migration-ledger regression/fix |
| `python3 scripts/smoke_hook_auth.py` | 29 passed |
| `python3 scripts/smoke_hook_offline.py`, `python3 scripts/smoke_outbox.py` | 23 and 32 passed |
| `python3 scripts/test_download_embedding_model.py` | 2 passed; checks checksum rejection, atomic publication and verified reuse without networking |
| `scripts/smoke_full_flow.sh`, `scripts/smoke_layer3.sh`, `scripts/smoke_hook.sh`, `scripts/smoke_mcp.sh` | Passed |
| `python3 scripts/validate_phase0.py` | Passed |
| `actionlint` on CI/release workflows | Passed, with optional shellcheck/pyflakes integrations disabled |
| Production image build and runtime | Native `linux/arm64` build passed; real semantic recall passed with `--network none` |

Core and PostgreSQL coverage includes entity/tag lookup, title priority,
one-hop links, person-scoped source IDs, exact RRF/type/pinned constants,
vector validation/provenance, persisted vectors, stale-input rejection,
forgetting and visibility. The existing PostgreSQL 512-window/omission and
why-changed ordering tests pass with hybrid ranking. The server tests load
the pinned FP32 model, backfill notes saved before model startup, index later
saves, recover a query without lexical overlap on search/smart-search/context/
recall/why-changed, and retain personal/project/global authorization and filters.
The file job persistence test verifies failed writes remain pending and retry.
A migration-ledger assertion failed before adding the missing migration entry,
then passed on a fresh database. The interrupted worker's retained core
compile-red log predates the embedding APIs; its PostgreSQL test-authoring
chronology is not reconstructed here.

The first resumed image build exposed missing ONNX C/C++ symbols on bookworm;
the final image builds/runs on `debian:trixie-slim`. The approved release split
keeps static musl CLI assets and uses GNU Linux server assets. Both amd64 and
arm64 image jobs remain configured; amd64 and Windows release builds were not
executed locally. Initial smoke attempts blocked behind cold concurrent Cargo
builds and timed out; the recorded passing rerun prebuilt the server first.
The smoke readiness window was not shortened or changed.

Resource measurements used the final production image and a clean `b9e3fdd`
production-image build, both native arm64, in a Colima VM with 2 CPUs/4 GiB RAM.
All three runtime probes used `--network none`, one synthetic note, and 32
queries; CPU is process ticks divided by queries, with 10 ms tick resolution.
These measure resource cost, not retrieval quality or production throughput.

| Measurement | Baseline | Hybrid, missing model | Hybrid, loaded model |
| --- | --- | --- | --- |
| RSS after queries | 5,936 KiB | 11,748 KiB | 155,064 KiB |
| CPU per query | about 0.31 ms | about 0.63 ms | about 5.63 ms |
| Health `semantic_search` | disabled | disabled | enabled |

The model files occupy 91,102,069 bytes. Summed uncompressed image layers grew
from 135,151,616 to 267,063,296 bytes (128.89 to 254.69 MiB; +125.80 MiB).
The base layer alone grew from 107,786,240 to 109,563,904 bytes (+1.70 MiB).
Docker's engine-reported `.Size` values were 37,780,318 and 136,719,363 bytes;
these are recorded separately from uncompressed layer totals. Loaded-model
RSS adds about 140 MiB over the same image with no model loaded.

An OSV audit of all 314 registry packages in `Cargo.lock` reports the newly
introduced `paste` 1.0.15 maintenance advisory `RUSTSEC-2024-0436`, pulled by
the approved embedding stack. It also reports `libsqlite3-sys` 0.30.1
(`RUSTSEC-2026-0459`) and `rsa` 0.9.10 (`RUSTSEC-2023-0071`); both already
existed at `b9e3fdd` and neither appears in the active workspace dependency
tree. This is an advisory inventory, not a clean security-audit claim.

No web files changed, so web checks/browser inspection were not run. No
independent helper review was run in this single-worker dispatch. No retention,
feedback, brief injection, instruction lifecycle, external embedding API or
recall-quality benchmark was added. Release-hook setup can still emit the
existing 404 warning for unreleased 0.1.4 assets while the local MCP smoke passes.

## v0.1.4 stage 4 — Session brief and agent instructions

Verified on 2026-11-17 on macOS, branch `v0.1.4`, with isolated file stores,
temporary agent homes and a scratch pgvector PostgreSQL 17 container. No live
Cairn server or real agent configuration was used.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`:
  passed.
- `cargo check --workspace --locked`: passed.
- `cargo test --workspace --locked`: 191 passed, 0 failed, 34 ignored. Ignored
  tests ran separately below.
- `scripts/ci_postgres_tests.sh`: 34 passed, 0 failed, one fresh database per
  test; includes both adapters' real-model tests and the new authenticated brief
  visibility scenario. Used the requested Colima socket, scratch Docker config
  and pinned MiniLM artifacts. Zero `cairn_ci_*` databases remained before the
  auto-remove container was stopped.
- Python hook-auth: 29 passed; offline capture/outbox: 23 and 32 passed.
- `scripts/test_agent_hooks.py`: 7 passed. Covers short start plus brief, full
  first-work instruction once, concurrent first-work delivery, resume, no-work
  and after-new-work Stop behavior, recursion suppression, marker exclusion,
  offline instruction delivery, stored credentials, once flags across 260 open
  sessions without payload/token text, and explicit assistant-capture opt-in.
- `scripts/test_agent_setup.py`: 10 passed. Covers JSONC comments, trailing
  commas, nested objects, strings with braces/comment markers, missing-entry
  no-op, duplicate/malformed input, native-CLI availability, a fresh Codex
  directory, all three unmarked-name conflict paths and unrelated config.
- Real `cairn setup --agent` integration test passed with native CLI stand-ins
  and temporary HOME directories. Isolated probes of actual Codex 0.169.0 and
  OpenCode 2.1.4 verified native install/reinstall/removal plus TOML/JSONC comment
  preservation; neither probe started a model or memory server.
- `smoke_full_flow.sh`, `smoke_mcp.sh`, `smoke_hook.sh` and
  `validate_phase0.py`: passed. CI/release `actionlint` passed with optional
  shellcheck/pyflakes integrations disabled. Web files were unchanged; web
  checks were not run.

Tests were written failing before the brief, hook lifecycle, setup and JSONC
implementations. Additional failing regressions caught instruction loss on an
offline capture, assistant opt-in being skipped by Stop handling, a missing
Codex home directory, and reinjection after session-state eviction. Each passed
after the shared fix. The first hook-auth run
timed out behind overlapping Cargo builds; the final passing run prebuilt the
server and ran sequentially, with no readiness timeout change.

Implementation: `crates/core/src/brief.rs` and sibling tests, both server brief
routes, `crates/server/tests/project_model.rs`, `crates/client/src/agents.rs`,
setup CLI/integration tests, `scripts/cairn_setup.py`, shared instruction text,
and the hook lifecycle adapter. The [hook contract](contracts/hook-v1.md#verified-agent-capabilities)
records per-agent official documentation and capabilities. All four stage 4
checklist items (3, 8, 9, 10) are implemented with the documented OpenCode
instruction-file fallback. No retention, feedback or new LLM behavior is claimed.
