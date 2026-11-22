---
id: CM-PLAN-019
title: "0.1.9 write-side plan"
status: frozen
---

# 0.1.9 write-side plan

Seventeen items plus one guard, all shipping in 0.1.9 (branch `0.1.9`, PR #17). Sources: the advisory v2
(ideas A1–A7, A9, D5, D6, D9, G1, G3, J1, J2, B1, B2/B3, I4), the Kivori investigation,
[`bench/analysis-0.1.9.md`](../../../bench/analysis-0.1.9.md) and [the 0.1.9 release notes](../../release-notes-0.1.9.md).

## Binding decisions (from the user; every spec relies on them)

- D-LLM: no LLM inside Cairn. The host agent is the only LLM. Local small models are allowed; a cross-encoder file ships only if the bench proves it (item 10).
- D-DATA: no data loss. Every schema change is an additive migration with a backfill of existing rows. Never DROP/DELETE/TRUNCATE user data, never a fresh-DB release.
- D-ANY: works with and without git; nothing is Orca-specific.
- D-TOK: minimise tokens; every spec states its token cost.
- D-RANK: new automatic notes start at low rank (`candidate`) and become `normal` when used or confirmed.
- D-DIGEST: session digests are project-visible.
- D-TOOLS: a 6th MCP tool only if needed (no item needs one).
- D-SHIP: everything ships in 0.1.9.

## Rules for implementers and reviewers (anti-drift)

1. **The spec is frozen.** Do not add, widen or "improve" scope. A gap or contradiction goes to the coordinator as a question; do not decide it in code.
2. **Test first.** Write the tests named in "Tests to write first", run them, paste the failing output, then implement until they pass, and paste the passing output.
3. **Touch only Allowed files.** A needed file not on the list is an escalation, not a judgment call. Generated files (`apps/web/lib/generated/*`) may change only by regeneration.
4. **Evidence** in every report: `file:line` for each change and pasted test output (failing then passing). "Should work" is not evidence.
5. **One item per worker.** One writer per file at a time; items that share a file run in order (see the checklist).
6. **Max two fix rounds.** After a reviewer's second round of findings on the same item still fails, escalate with the evidence.
7. **No data loss.** Migrations are additive (`ADD COLUMN ... DEFAULT`, `CREATE TABLE`, `CREATE INDEX`, `INSERT ... SELECT` backfill). Item 00's guard enforces it in CI. A backfill must be idempotent and covered by a test on a database that already has rows.
8. **Tokens.** Anything an agent reads (hook text, tool schema, tool reply) is measured in bytes in a test and stays within the spec's budget.
9. Reviewers check the diff against the Acceptance checks one by one and report each as pass/fail with evidence; style opinions outside the spec are not findings.

## Ordered checklist

Migration numbers are fixed here so parallel work does not collide.

| # | Item | Depends on | Migration |
|---|---|---|---|
| 00 | [x] [Migration guard](00-guard.md) | — | — |
| 01 | [x] [Distilled notes take project scope, low rank](01-project-scope.md) | 00 | `0007_memory_rank` |
| 02 | [x] [Project recognition everywhere](02-project-recognition.md) | 00 | — |
| 12 | [x] [Subagents and automated sessions](12-automation-sessions.md) | 00 | — |
| 05 | [x] [Handoffs that are not overwritten](05-handoffs.md) | 02, 12 | — |
| 04 | [x] [Session digest note](04-session-digest.md) | 01, 05 | — |
| 03 | [x] [Ask once to save 0–3 notes](03-save-ask.md) | 12 | — |
| 17 | [Always-active memory: no expiry, relations and ranking](17-expiry-keeps-data.md) (high priority: live data loss) | 00, 04, 05 | `0012_always_active` (index only) |
| 06 | [User corrections become rules](06-corrections.md) | 03 | — |
| 11 | [Proof count and rare-token veto](11-proof-and-veto.md) | 01 | `0008_proof_count` |
| 09 | [Aliases, backfill, regenerated bench](09-aliases.md) | 03, 11 | `0009_memory_aliases` |
| 08 | [Per-project web report](08-project-report.md) | 01, 04, 12 | `0010_session_outcome` |
| 07 | [Write-side bench in CI](07-write-bench.md) | 01–06, 12 | — |
| 10 | [Cross-encoder experiment](10-cross-encoder.md) | 09 | — (no schema) |
| 13 | [Fixes: recording](13-fixes-recording.md) | 03, 08, 09, 12 | `0011_fixes` |
| 14 | [Fixes: detectors](14-fixes-detectors.md) | 11, 13 | — |
| 15 | [Fixes: admin web page and Apply](15-fixes-web.md) | 14 | — |
| 16 | [Fixes: device fixes, `cairn fixes`, audited auto-join](16-fixes-cli-autojoin.md) | 15 | — |

Shared files, in this order only: `crates/client/src/hook/mod.rs` and `hook/state.rs` (02 → 12 → 05 → 03 → 06);
`crates/core/src/process/distill.rs` (01 → 05 → 04 → 17 → 08); `crates/store/src/capture.rs` (01 → 05 → 04 → 12 → 17 → 08);
`crates/client/src/tools.rs` and `crates/server/src/routes/memory.rs` (03 → 09); `crates/core/src/retrieve/rank.rs` (01 → 17 → 11 → 14);
`crates/store/src/lib.rs` migration list (01 → 17 → 11 → 09 → 08 → 13; 17 appends `0012` last, numbers 0008–0011 slot in before it);
`crates/store/src/fixes.rs` (13 → 14 → 15 → 16); `crates/store/src/memory.rs` (17 → 09 → 15 → 16); hook files after 06 (→ 13).

## Global acceptance (before the PR merges)

1. `python3 scripts/check_migrations.py` passes (item 00) and its own test passes.
2. `cargo fmt --all -- --check`.
3. `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.
4. `CAIRN_TEST_REQUIRE_DB=1 cargo test --workspace --locked` against a real Postgres + pgvector.
5. Web: `npm run typecheck && npm run lint && npm run build && npm test` in `apps/web`; generated types current.
6. Smokes: `./scripts/smoke_mcp.sh` and `./scripts/smoke_web.sh`.
7. `python3 scripts/validate_phase0.py`, `check_update_assets.py`, `check_web_version.py`.
8. Bench floors: v2 hit@5 ≥ 0.65 with 0 weak hits; v1 hit@5 ≥ 0.88, link ratio ≥ 0.60; the write bench (item 07) passes.
9. Upgrade test: a database migrated at 0.1.8 with rows in every table upgrades to 0.1.9 with the same row counts and no changed `memory_id`.
10. Release notes `docs/release-notes-0.1.9.md` updated with each item's user-visible change and token cost (done by the integrator, not by item workers).

## BLOCKING questions

None. Non-blocking questions with a stated default are in items 01, 06, 09 and 12; the default applies unless the user answers otherwise.
