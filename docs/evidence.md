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

## Script consolidation (2026-11-18)

The per-layer smoke scripts were merged into `scripts/smoke_full_flow.sh`, `scripts/smoke_web.sh`, `scripts/smoke_mcp.sh`, and `scripts/smoke_postgres.sh`. Those four run in the `Acceptance smokes` CI job. The job does not download the embedding model. `crates/server/src/main.rs` keeps lexical, entity, and link recall when `--embedding-model-dir` is absent. Historical sections below keep the command that ran that day.

| Removed script | Where the assertion lives now |
| --- | --- |
| `smoke_layer1.sh` CLI start, capture, end, context recall, resume id, why-changed source event, forget, lesson, correct | `smoke_full_flow.sh`. Scope `worktree` is obsolete: ADR 0014 item 0 leaves an unattached capture in `personal`, which the smoke asserts. |
| `smoke_layer1.sh` repeated why-changed order | `smoke_full_flow.sh` compares two why-changed responses. |
| `smoke_rest.sh` unauthenticated start, observe, end, context source event, forget, empty search | Authenticated equivalent is `smoke_full_flow.sh`. The file router still accepts a missing bearer before an account exists; `capture_end_context_and_forget_work_over_http` in `crates/server/src/lib.rs` exercises that path. |
| `smoke_hook.sh` accepted hook event, session-end search of the summary and handoff | `smoke_full_flow.sh`, plus CI `scripts/smoke_hook_auth.py` and `scripts/smoke_hook_offline.py`. |
| `smoke_export.sh` same-account import counts of zero and a forgotten id kept out of recall | `smoke_full_flow.sh` CLI re-import, and the HTTP restore already in that script. `export_and_import_round_trip_over_http` covers the HTTP round trip. |
| `smoke_viewer.sh` viewer markers, removed share/team controls, empty diagnose counts | `smoke_full_flow.sh`. The same markers are in `viewer_page_is_available_from_the_server`. |
| `smoke_layer3.sh` outbox redaction, offline pending, revoked purge | CI `scripts/smoke_outbox.py` (`test_queue_file_bytes_omit_synthetic_content_secrets`, `test_queue_policy_replay_order_and_auth_purge`). |
| `smoke_layer3.sh` duplicate push, cross-device pull, cursor, rotate, revoke | `smoke_postgres.sh`. Cross-person event-id rejection is `sync_event_id_collision_cannot_leak_another_persons_observation` and `file_sync_adopts_an_owned_observation_once`. |
| `smoke_layer4_cli.sh` project decision fields, verify, correct keeping scope and alternatives, freshness verification | `smoke_full_flow.sh`. |
| `smoke_layer5_cli.sh` init join, member visibility of project capture and summary, personal hidden, global visible, reader cannot correct or delete, promote and retired commands stay gone, delete hides the row | `smoke_full_flow.sh`. Promote is removed by ADR 0014 item 0 and `docs/contracts/project-layer4.md` ("Retired surfaces"). |
| `smoke_postgres.sh` promote, `promoted_from`, preview, and the `promotion` freshness action | Replaced in the same script by a direct project save (ADR 0014 item 0). Sync, devices, verify, merge, entities, maintenance, member visibility, and retired project routes stay. |
| `measure_layer2.py` why-changed reason and `file_contains_symbol` | `why_changed_reports_reason_relationships_staleness_and_contradictions` and `why_changed_route_returns_reason_relationships_and_conflicts`. |
| `measure_layer2.py` branch memory absent from `main` and present in the matching context | `smoke_full_flow.sh` and `scoped_search_keeps_branch_memory_out_of_main_context`. |
| `measure_layer2.py` stale freshness below 50 and lexical `contradictory` / `contradiction_ids` | The same core recall test. ADR 0014 stage 7 keeps the lexical signal and adds declared `contradicts`, which `smoke_full_flow.sh` posts. |
| `measure_layer2.py` repeated-investigation id stability | `smoke_full_flow.sh`. |
| `check_detail_actions.mjs` promote preview and publication posts | Obsolete. ADR 0014 item 0 and the retired surfaces in `project-layer4.md`. The console source no longer contains those actions; `smoke_web.sh` fails if they return. |
| `check_detail_actions.mjs` delete copy, reload sentence, verify, merge, close, member visibility, login and logout | `smoke_web.sh` (action copy, the reload sentence in `project-detail.tsx`, verify, merge, close, member list). Account reset and password change are `file_admin_lists_accounts_and_resets_a_password` and `file_password_change_needs_the_current_password_and_is_rate_limited`. |
| `check_detail_actions.mjs` dialog timing (no POST before confirm, phone overflow, theme class) | Not re-run. The script required a machine-local Playwright install, which is not a repository dependency and is not a reliable GitHub Actions job, and its fixture called the removed promote route. |
| `graph_canvas_shots.mjs` | Deleted. No repository reference. |
| `smoke_agent_lib.sh` | Kept. It is the account and device harness for `smoke_mcp.sh`, and it unsets `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`, `CAIRN_CONFIG`, and `CAIRN_OUTBOX`. |

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

## v0.1.4 stage 5 — retention and feedback

Verified on 2026-11-17 on macOS, branch `v0.1.4`. No live Cairn server was
contacted. Verification shells cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN` and
`CAIRN_OUTBOX`. PostgreSQL used a disposable `pgvector/pgvector:pg17` container
on the Colima socket with a scratch Docker config, one database per ignored
test, then the container was removed.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features --offline -- -D warnings`: passed.
- `cargo test --workspace --offline`: 196 passed, 0 failed, 35 ignored.
- `scripts/ci_postgres_tests.sh`: 35 passed, 0 failed, including
  `postgres_retention_and_feedback_match_the_file_store` and both real-model
  semantic tests. Scratch databases were dropped with the container.
- `scripts/smoke_hook_auth.py`: 29 passed.
- `scripts/smoke_hook_offline.py`: 23 passed.
- `scripts/smoke_outbox.py`: 32 passed.
- `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh` and
  `scripts/validate_phase0.py`: passed.
- Web `typecheck`, `lint`, `build` and `scripts/smoke_web.sh`: passed.

The approved multipliers live in `crates/core/src/retention.rs` and the
[continuity contract](contracts/continuity-v1.md#hybrid-recall-v014). A score
below 0.45 lowers rank only. Captures, session summaries and handoffs decay.
Rules, decisions, gotchas, procedures, facts, preferences, explicit saves,
lessons and corrections stay at factor 1. One helpful or stale vote per person
is shared for project and global memories and owner-only for personal memory.
Returned search, context and why-changed hits reinforce access stats. The
separate temporary-observation expiry sweep is unchanged.

### Stage 5 review corrections

Verified on 2026-11-17 on macOS, branch `v0.1.4`, with the same isolated
PostgreSQL setup and cleared live-server environment described above.

- Feedback visibility: `feedback_rejects_every_memory_hidden_from_reads` and
  its `postgres_` counterpart cover forgotten memories and source events,
  superseded status, expiry and live successors. PostgreSQL first failed by
  returning a forgotten memory; the file gate already passed. Both now pass.
- Concurrent votes: `postgres_concurrent_feedback_preserves_every_persons_vote`
  coordinates two blocked voters with a held row lock. It first persisted one
  accepted vote instead of two; it now retains both identities and counts.
- Summary state: `summary_regeneration_preserves_reinforcement_and_feedback`
  and its `postgres_` counterpart first reset access count from 1 to 0.
  Both now preserve access count, last access time, votes and both counts
  across delayed-capture regeneration, with stale and helpful votes.
- Import: `import_discards_foreign_votes_and_untrusted_counts` and its
  `postgres_` counterpart first retained forged votes or counts. Both now
  clear votes, zero derived counts and permit idempotent repeat import.
  PostgreSQL also asserts the raw stored count columns are zero.
- Existing tests now assert stale replacement before helpful replacement,
  compare the owner's score before and after a member's vote, and compare
  PostgreSQL ranking, reinforcement and feedback against a real file store.

Final checks: `cargo fmt --all -- --check`, `cargo check --workspace`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo test --workspace` (199 passed, 39 ignored),
`scripts/ci_postgres_tests.sh` (39 passed), `scripts/smoke_full_flow.sh`,
`scripts/smoke_mcp.sh` and `python3 scripts/validate_phase0.py` passed.
The first PostgreSQL suite run passed 37 tests and failed its two semantic
checks because the model-directory variable was unset. The final complete
run used the checksum-verified pinned MiniLM model and passed both semantic
checks. MCP setup emitted a nonfatal hook-script download HTTP 404 warning;
the MCP flow passed. Web files were unchanged, so web checks were not rerun.

## v0.1.4 stage 6 — durable jobs and quarantine

Verified on 2026-11-17 on macOS, branch `v0.1.4`. No live Cairn server was
contacted. Verification shells cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN` and
`CAIRN_OUTBOX`. PostgreSQL used a disposable `pgvector/pgvector:pg17` container
on the Colima socket with a scratch Docker config, one database per ignored
test, then the container was removed.

- `cargo fmt --all -- --check`: passed before the final formatting pass, which
  was applied and included in the PostgreSQL run.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace`: 209 passed, 0 failed, 40 ignored.
- `scripts/ci_postgres_tests.sh`: 40 passed, 0 failed, including
  `postgres_jobs_lease_backoff_quarantine_and_exclusion`, the idle-sweep
  quarantine case, and both real-model semantic tests. The pinned MiniLM
  directory was a scratch download checked by `scripts/download_embedding_model.py`.
- `scripts/smoke_hook_auth.py`: 29 passed.
- `scripts/smoke_hook_offline.py`: 23 passed.
- `scripts/smoke_outbox.py`: 32 passed.
- `scripts/test_agent_hooks.py`: 7 passed.
- `scripts/test_agent_setup.py`: 10 passed.
- `scripts/smoke_hook.sh`, `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh`
  and `scripts/validate_phase0.py`: passed. MCP setup still emits the nonfatal
  HTTP 404 for unreleased 0.1.4 hook assets.
- Web `typecheck`, `lint`, `build` and `scripts/smoke_web.sh`: passed, including
  the admin Jobs page and a non-admin 403 on `admin/jobs`.

Job behavior was tested on the file store and on PostgreSQL: exclusive claim,
lease expiry, backoff, permanent and transient errors, supersede, quarantine,
requeue, discard, and a lost claim. `POST session/end` still returns the summary
after the notes are stored. Public `/health` is unchanged.

### Stage 6 review fixes (2026-11-17)

The independent review of `c472cf9` was fixed on `v0.1.4` before stage 7.
Both stores now keep one pending row when a retry or expired lease meets a
successor, refuse to claim that successor while the lease is live, and skip
quarantined keys in reconciliation and the idle sweep until an admin requeues
or a newer observation arrives. A lost distill claim does not commit the
summary or clear a newer capture's pending mark. PostgreSQL queue changes are
row updates. The backoff schedule is 30, 60, 120, and 240 seconds across five
total attempts. The same shell rules applied: cleared Cairn environment
variables, no live server, disposable `pgvector/pgvector:pg17` on the Colima
socket, and `CARGO_TARGET_DIR=/tmp/cairn-e2e-20261117-target`.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace`: 213 passed, 0 failed, 47 ignored.
- `scripts/ci_postgres_tests.sh`: 47 passed, 0 failed, including the new job
  regressions and both real-model semantic tests. The pinned MiniLM directory
  was `/tmp/cairn-stage6-fix-minilm`, verified by
  `scripts/download_embedding_model.py`.
- `scripts/smoke_hook_auth.py`: 29 passed. `scripts/smoke_hook_offline.py`: 23
  passed. `scripts/smoke_outbox.py`: 32 passed.
- `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh`, and
  `scripts/validate_phase0.py`: passed. MCP setup still emits the nonfatal
  HTTP 404 for unreleased 0.1.4 hook assets.
- Web files were unchanged, so web checks were not rerun.
- One enqueue beside retained succeeded rows took 4.1 ms at 100 rows, 4.2 ms
  at 1,000, and 10.2 ms at 5,000. Those retained row identities did not change.

## v0.1.4 stage 7 — contradiction handling

Checked on 2026-11-17. A declared `contradicts` link shades only the contradicted
memory by `max(0.25, 1/(1+n))` after the stage-3 and stage-5 score, for a viewer
who can read both ends. The similarity job `similarity-scan` reviews cosine
0.40 through 1.0, labels below 0.75 as `possible_conflict` and 0.75 and above
as `near_duplicate`, inspects the 60 coldest non-capture memories, and inserts
at most 25 open reviews per hourly run. Explicit supersede writes `supersedes`
on the existing successor. Reviews are omitted unless the caller can read both
memories. Edit rights gate declare, dismiss, and supersede.

`postgres_quarantine_cleanup_keeps_a_newer_captures_pending_marker` again
asserts exactly 3 memories. One `Decision:` capture derives the session
summary, the handoff, and that typed note. The first PostgreSQL script run
failed only the two semantic tests, because `CAIRN_TEST_EMBEDDING_MODEL_DIR`
was unset. The rerun below includes them.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace`: 217 passed, 0 failed, 48 ignored.
- `scripts/ci_postgres_tests.sh`: 48 passed, 0 failed, including
  `postgres_contradiction_shades_reviews_and_supersede_hides`, the exact
  quarantine count, and both real-model semantic tests. The pinned MiniLM
  directory was `/tmp/cairn-stage7-minilm`.
- `scripts/smoke_hook_auth.py`: 29 passed. `scripts/smoke_hook_offline.py`: 23
  passed. `scripts/smoke_outbox.py`: 32 passed.
- `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh`, and
  `scripts/validate_phase0.py`: passed. MCP setup still emits the nonfatal
  HTTP 404 for unreleased 0.1.4 hook assets.
- Web `typecheck`, `lint`, and `build` passed, and `scripts/smoke_web.sh` passed.
- Shells cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`, and `CAIRN_OUTBOX`. No live
  server. PostgreSQL was disposable `pgvector/pgvector:pg17` on the Colima socket.

### Stage 7 review fixes (2026-11-17)

The independent review of `3930784` was fixed on `v0.1.4`. Dismiss now requires
edit rights on both memories. A project supersede hides the target when the
successor is another member's published memory. Re-deriving a summary keeps
`contradicts` and `supersedes`, and new distilled notes do not copy those
links. Project declare, dismiss, and supersede take a shared lock on the
project row before they authorize the write. A similarity scan publishes
reviews in the same transaction as its claim, drops a pair that was declared
or superseded before commit, and publishes nothing when the claim is gone.
Cosine values a few ulps past 1 stay inside the near-duplicate band.

Each new test failed on the previous code, then passed. File storage already
hid a cross-author project supersede and already refused an expired similarity
claim; those two file tests were green before the fix. The PostgreSQL copies
of both were red.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace`: 223 passed, 0 failed, 55 ignored.
- `scripts/ci_postgres_tests.sh`: 55 passed, 0 failed, including the seven new
  PostgreSQL regressions and both real-model semantic tests. The pinned MiniLM
  directory was `/tmp/cairn-stage7-fix-minilm`.
- `scripts/smoke_hook_auth.py`: 29 passed. `scripts/smoke_hook_offline.py`: 23
  passed. `scripts/smoke_outbox.py`: 32 passed.
- `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh`, and
  `scripts/validate_phase0.py`: passed. MCP setup still emits the nonfatal
  HTTP 404 for unreleased 0.1.4 hook assets.
- Web files were unchanged, so web checks were not rerun.
- The retained-job enqueue check printed 3.8 ms at 100 rows, 4.2 ms at 1,000,
  and 3.5 ms at 5,000. The review's unbounded similarity read was not one of
  the eight findings and was not changed or benchmarked separately.
- Shells cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`, and `CAIRN_OUTBOX`. No live
  server. PostgreSQL was disposable `pgvector/pgvector:pg17` on the Colima socket.

## v0.1.4 stage 8 — memory link graph

Checked on 2026-11-17. Typed links `relates`, `causes`, `fixes`, and
`depends_on` are stored on the source memory in both stores. Create and remove
need read access to both ends, the same scope and project, and edit rights on
the source. Recall walks those links one hop inside the existing link channel.
The relation does not change the channel weight. `contradicts` and `supersedes`
stay on their own fields and are not hops. The graph draws them only when both
ends are still in the readable recalled set, so a live supersede hides its
edge. The viewer cap defaults to 120 and may be set from 120 through 500.
Sigma.js 4.0.6 and graphology 0.26.0 draw `/graph`. There is no code graph.

The new file-store and PostgreSQL tests passed with the implementation. A
separate red run was not archived.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace`: 227 passed, 0 failed, 56 ignored.
- `scripts/ci_postgres_tests.sh`: 56 passed, 0 failed, including
  `postgres_links_permissions_visibility_cap_and_one_hop` and both real-model
  semantic tests. The pinned MiniLM directory was `/tmp/cairn-stage7-fix-minilm`.
- `scripts/smoke_hook_auth.py`: 29 passed. `scripts/smoke_hook_offline.py`: 23
  passed. `scripts/smoke_outbox.py`: 32 passed.
- `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh`, and
  `scripts/validate_phase0.py`: passed. MCP setup still emits the nonfatal
  HTTP 404 for unreleased 0.1.4 hook assets.
- Web `typecheck`, `lint`, and `build` passed, and `scripts/smoke_web.sh` passed.
- Playwright screenshots, looked at after the node-size fix:
  `/tmp/cairn-stage8-graph/graph-light-1440.png`,
  `/tmp/cairn-stage8-graph/graph-dark-1440.png`,
  `/tmp/cairn-stage8-graph/graph-light-390.png`,
  `/tmp/cairn-stage8-graph/graph-dark-390.png`.
  Light and dark both show scope colours and relation edges. At 390px the
  filters stack and the canvas pans; a couple of labels sit against the edge.
  The dark-canvas claim in this list does not hold: the 2026-11-17 review
  found black labels and invisible global nodes. The fix is recorded below.
- Pinned canvas packages, all MIT: `sigma` 4.0.6 (3.8 MB on disk),
  `graphology` 0.26.0 (2.7 MB), `graphology-layout-forceatlas2` 0.10.1 (104 KB).
  Sigma also installs `graphology-utils` (136 KB) and `graphology-types` (68 KB).
- Shells cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`, and `CAIRN_OUTBOX`. No live
  server. PostgreSQL was disposable `pgvector/pgvector:pg17` on the Colima socket.

## Stage 8 review fixes (2026-11-17)

The review of `50b4866` (1 P1, 5 P2, 2 P3) is fixed here.

- Dark colours are read back through a canvas, so a `lab()` theme value is not
  replaced by the light fallback. Labels use the foreground. The canvas
  rebuilds when the document `dark` class changes. Global nodes use that
  foreground, so they stay visible on a dark card.
- Import drops a link that creation would reject: a self-link, a duplicate
  triple, or a target that is missing, unreadable, or in another scope or
  project. `dropped_links` lists them. Recall and the graph ignore a stored
  link that fails the same scope and project rule. File and PostgreSQL both
  call `sanitize_imported_links`.
- Canvas edge keys are length-prefixed, so a memory id that contains `:` does
  not collide with another triple.
- Labels sit above the node on a card-coloured chip. A long canvas label is
  shortened; the link table keeps the full title.
- The flag that swallowed the click after a drag is gone. Sigma already drops
  the click that ends a drag.
- The page has a keyboard-reachable table of source, relation, and target.
- Scope, project, and relation use the shared Select. The node cap stays the
  shared Input.
- Disconnected pairs are packed in a centered row.

Checked on 2026-11-17. Shells cleared `CAIRN_URL`, `CAIRN_DEVICE_TOKEN`, and
`CAIRN_OUTBOX`. No live server. PostgreSQL was disposable
`pgvector/pgvector:pg17` container `cairn-stage8-fix-pg` on the Colima socket,
removed after the suite. The Cargo target was `/tmp/cairn-stage8-fix-target`.
The pinned MiniLM directory was `/tmp/cairn-stage7-fix-minilm`.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace`: 230 passed, 0 failed, 56 ignored.
- `scripts/ci_postgres_tests.sh`: 56 passed, 0 failed, including
  `postgres_links_permissions_visibility_cap_and_one_hop` and both real-model
  semantic tests.
- `scripts/smoke_full_flow.sh`, `scripts/smoke_mcp.sh`, and
  `scripts/validate_phase0.py`: passed. MCP setup still emits the nonfatal
  HTTP 404 for unreleased 0.1.4 hook assets.
- Web `typecheck`, `lint`, and `build` passed, and `scripts/smoke_web.sh` passed.
- Node test `apps/web/lib/graph-edge-key.test.ts` passed. Playwright click
  after a drag passed, the link table opened a memory from the keyboard, and
  the colon edge-key fixture drew a canvas.
- Screenshots, looked at after the fixes:
  `/tmp/cairn-stage8-fix-shots/graph-sparse-light-1440.png`,
  `/tmp/cairn-stage8-fix-shots/graph-50-light-1440.png`,
  `/tmp/cairn-stage8-fix-shots/graph-50-dark-1440.png`,
  `/tmp/cairn-stage8-fix-shots/graph-50-light-390.png`,
  `/tmp/cairn-stage8-fix-shots/graph-50-dark-390.png`,
  `/tmp/cairn-stage8-fix-shots/graph-500-light-1440.png`,
  `/tmp/cairn-stage8-fix-shots/graph-500-dark-1440.png`,
  `/tmp/cairn-stage8-fix-shots/graph-500-light-390.png`,
  `/tmp/cairn-stage8-fix-shots/graph-500-dark-390.png`,
  `/tmp/cairn-stage8-fix-shots/live-theme-switch.png`.
  Dark labels and global nodes are visible. The six-node graph is a centered
  row with separated labels. At 500 nodes the desktop canvas shows distinct
  circles; a narrow viewport still stacks a long chain, and the link table
  lists every connection. A few dense labels still share a line.

## v0.1.5 release verification (2026-11-18)

Checked on macOS, uncommitted `main` at `8e20409`, with `CAIRN_URL`,
`CAIRN_DEVICE_TOKEN`, `CAIRN_CONFIG`, and `CAIRN_OUTBOX` unset. No release,
version bump, commit, or push. Stores were disposable. The detailed fixture
log is `/tmp/cairn-v015-point5/report.json` on this machine. It contains
synthetic markers only.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: passed.
- `cargo test --workspace --locked --no-fail-fast`: 251 passed, 0 failed, 60 ignored.
  Before the handoff assertion below, `file_http_session_distillation` failed
  because a forgotten next-step capture no longer leaves an empty handoff.
- `scripts/ci_postgres_tests.sh`: the first run, before the expectation
  updates, was 60 tests and 3 failed. After those updates, one fresh
  `pgvector/pgvector:pg17` container (`cairn-v015-ci-pg2`, host port 32780)
  ran the whole script again: **60 PostgreSQL tests, 0 failed**. The pinned
  MiniLM directory `/tmp/cairn-stage9-fix-minilm` was reused and its checksums
  matched. This machine has no host `psql`; a `docker exec` wrapper supplied
  it. The container was removed after the run. No Rust files changed in that
  follow-up, so fmt and Clippy were not repeated.
- `scripts/smoke_mcp.sh`, `scripts/smoke_postgres.sh`, `scripts/smoke_web.sh`,
  and `scripts/smoke_full_flow.sh`: passed. The full-flow prose next step is
  now a user prompt, and the branch context query is `redis cache`, so the
  floor still returns that branch summary. The first web run failed because
  the shell still had `CAIRN_DATABASE_URL`; the clean rerun passed.
- `python3 scripts/test_agent_hooks.py`: 14 passed.
  `python3 scripts/test_agent_setup.py`: 12 passed.

Direct `cairn mcp` against a loopback file server and a fresh PostgreSQL
database both returned four-field `memory_save` and `memory_lesson_save`
receipts, object list results, one decision card among three distractors,
`memory_get` of that decision, an empty `config reload` search, and the raw
capture only after `includeCaptures`. CLI search stayed pretty server JSON.
REST get and account export stayed full records. Graph text was tab-delimited
TOON; its `structuredContent` stayed JSON.

Token counts use the already installed `gpt-tokenizer` 3.0.1
`countTokens` encoding `o200k_base`. No tokenizer was downloaded. On the file
fixture, the search card text was 64 tokens and its `structuredContent`
compact form was also 64. Pretty JSON of that same card was 90 (compact is
28.9% smaller). The CLI pretty server result for the same search was 594
tokens; the card is 89.2% smaller, and that gap is the dropped fields, not
the encoder. The context card was 90 text, 90 compact, and 126 pretty.
Graph text was 265 against 518 CLI pretty JSON (48.8% smaller) and 331
compact `structuredContent` (the TOON text is 19.9% smaller than that
object). A later file-store seed added three open global reviews whose
memories were visible. `memory_similarity_reviews` returned those three rows.
The TOON text was 175 tokens and kept `project_id` null. Compact
`structuredContent` was 207 tokens and dropped those nulls. Pretty JSON of
that same omitted object was 295 tokens, so compact is 29.8% smaller than
pretty JSON of the identical object; that pair is the encoding comparison.
The TOON text is 15.5% smaller than the compact object even though the text
still contains the nulls the object drops. CLI `cairn reviews` and REST
`GET /similarity-reviews` were both 351 pretty tokens and kept the nulls. No independent comprehension
check was run.

Fixture hook integration, not a native model session: a Claude checkpoint was
silent, Codex's later brief in the same checkout contained the identity and
the handoff marker, and the other worktree and the other account contained
neither. A `SessionStart` with `source` `compact` returned the fenced brief.

Native hosts were retried against listening loopback stubs with inherited
provider credentials removed. PostgreSQL and the token counts above were left
as recorded. The host captures are
`/tmp/cairn-v015-point5/followup/report-native.json` and
`/tmp/cairn-v015-point5/followup/report.json` on this machine.

Claude Code 2.1.327 `-p` used `--autocompact 100000`, a temporary
`CLAUDE_CONFIG_DIR`, and `ANTHROPIC_BASE_URL` on the Messages stub. It sent two
`POST /v1/messages` requests to that loopback. The stub streamed
`input_json_delta` for the advertised `Write` tool with an absolute
`file_path` and `content`. The tool result began `File created successfully`,
and `src/alpha-v015.txt` contained `ALPHA-V015`. `UserPromptSubmit` stored the
prompt, `PostToolUse` stored the Write payload, and Stop stored silent handoff
`587d133d-5fc4-4355-9dfb-47782496af9a`. No third request and no compaction
request followed the synthetic `usage.input_tokens` of 180000.

Codex CLI 0.170.0 `exec` used a temporary `CODEX_HOME`,
`wire_api = "responses"`, `--trust`,
`--dangerously-bypass-approvals-and-sandbox`, and
`--dangerously-bypass-hook-trust`. `hooks.json` was
`{"hooks": {"SessionStart": ...}}`. Stderr recorded SessionStart and
UserPromptSubmit completing, then `mcp: cairn/memory_save (completed)`,
`memory_lesson_save`, `memory_search`, and `memory_get`. Each function call
used namespace `mcp__cairn`. Search returned one decision card titled
`Host stub cedar_marker_v015`. `memory_get` returned that full record.
`exec_command` wrote `src/codex-marker.txt` with `codex-marker` and exited 0;
`PostToolUse` recorded it as `Bash`, and Stop completed. The cedar decision
and birch lesson were in the disposable store. Stderr did not mention
`api.openai.com`. `exec resume --last` with
`model_auto_compact_token_limit=200` reported `tokens used 112` and ran
SessionStart, UserPromptSubmit, and Stop, with no PreCompact or PostCompact.
The Codex SessionStart brief contained the Claude handoff next step for
`ALPHA-V015`; the resume brief also contained the cedar and birch notes.

OpenCode 2.1.4 direct mode kept `mcp.servers.cairn.codemode` false, denied
`execute`, and allowed `cairn_memory_save`, `cairn_memory_lesson_save`,
`cairn_memory_search`, and `cairn_memory_get` with resource `*`. One
`opencode serve` process stayed up for the scratch checkout.
`GET /api/mcp` with `x-opencode-directory` set to that checkout returned
`{"data":[]}`. The first prompt in session `ses_e109279f4ffeBStsy8sl3TpSYY`
advertised 11 built-in tools and no `cairn_*` name. The second prompt in that
same session advertised 49 tools, including those four Cairn names. The host
executed them: `memory_save` returned a global decision receipt,
`memory_lesson_save` returned a gotcha receipt, `memory_search` returned one
card titled `Host stub cedar_marker_v015`, and `memory_get` returned that full
record. Both memories were in the disposable store. The capture is
`/tmp/cairn-v015-point5/warm/report.json`. The installed 2.1.4 build still
omits the MCP registry on the first turn; this run does not treat a newer
release as fixed. Code Mode and a separate ping server were not needed after
that second turn. This is a host tool-list result, not model comprehension.

This MCP rerun did not compact. An earlier OpenCode 2.1.4 loopback session in
`/tmp/cairn-oc-report.json` did: its event list includes
`session.compaction.ended`, its hook log includes `SessionStart` with `source`
`compact`, and a later summary request still carried the brief and its next
step. Claude Code and Codex native probes in this point-5 pass did not emit a
compaction event. Their compaction check remains the fixture `SessionStart`
with `source` `compact`.

## v0.1.6 point 2 — routine cleanup and explicit saves (2026-11-19)

The console Memories page no longer requests the review queue on load. A
`Show near-duplicates` control fetches open reviews on demand and filters to
`band: near_duplicate`; the component and its explicit dismiss, contradict,
and supersede actions are reused. The similarity scan publishes only
`near_duplicate` pairs, so a 0.5 cosine pair creates no review. Historical
`possible_conflict` rows remain in storage and the API.

Explicit saves reuse one record for the same owner, scope, project, kind,
lifecycle, context, `supersedes`, and meaning-bearing fields. Evidence is
excluded from identity and merged; a no-op retry does not rewrite
`changed_at`. The file store scans owned live records. PostgreSQL takes a
transaction advisory lock keyed by owner and SHA-256 identity fingerprint,
compares the same identity in Rust, and updates only the record, `changed_by`,
and `changed_at`. No migration was added.

Checks on this machine (Andress-MacBook-Air):

- `cargo test --workspace --lib`: 239 passed (core 157, server 35).
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p cairn-core -p cairn-storage -p cairn-server --all-targets -- -D warnings`: passed.
- `npm run typecheck` and `npm run lint` in `apps/web`: passed.
- `./scripts/smoke_mcp.sh`: passed.
- `./scripts/smoke_web.sh`: passed.
- Disposable `pgvector/pgvector:pg17` container: all 56
  `cairn-storage --test postgres` ignored tests passed in fresh databases,
  including `postgres_identical_save_reuses_one_row_and_merges_evidence`,
  `postgres_concurrent_identical_saves_keep_one_row_and_all_evidence`, and
  `postgres_possible_conflict_cosine_publishes_no_review`; all 3
  `cairn-server --test project_model` ignored tests passed.
- The 2 `cairn-server --test semantic` ignored tests were not runnable:
  `CAIRN_TEST_EMBEDDING_MODEL_DIR` is unset (`NotPresent`). They are not part
  of this change's path.

No browser automation was used. The point-1 edit in
`scripts/cairn_instructions.txt` was left untouched.
