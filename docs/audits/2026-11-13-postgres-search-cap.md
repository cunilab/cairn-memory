---
id: CM-AUDIT-011
title: "PostgreSQL search cap — 2026-11-13"
status: superseded
---

# PostgreSQL search cap — 2026-11-13

Current ranking and cap behavior are owned by the
[hybrid recall contract](../contracts/continuity-v1.md#hybrid-recall-v014).

PostgreSQL `search` never returns more than 512 rows. This note checks whether that bound is a safe, honest limit or a hidden truncation in context, why-changed, resume, and unresolved. Read-only audit: no application, dependency, schema, or lock change. It does not review ranking, embeddings, lifecycle, or share grants.

## Progress

| Item | Status |
| --- | --- |
| Capture findings 1–7, choice B, device-only offline enqueue | Accepted in [CM-AUDIT-008](./2026-11-12-memory-capture.md). Not rechecked. |
| Account search limit, contradiction scope and eligibility | Fixed and accepted in [CM-AUDIT-009](./2026-11-13-account-search-limit.md) and [CM-AUDIT-010](./2026-11-13-recall-filter-eligibility.md). Not rechecked. |
| Old-data republishing, identity-field secrets and unsupported scanner patterns, account/anonymous offline enqueue, a preexisting cross-person `supersedes` link that can hide an owned memory | Still queued separately. Not expanded. |
| PostgreSQL 512-row cap | This note. Finding 1 fixed 2026-11-13 (omission count and hint, option 1). Finding 2 fixed 2026-11-13 (choice A: decision-first ranking before the cut), verified by the checks under finding 2. |

## Path (pre-fix reading)

This section and the Verdict, Trigger and Impact lines, Not defects, and the probe table in Evidence describe the code before the finding 1 fix. Finding 1's `Fixed` paragraph and check describe the current code.


- `MAX_SEARCH_RESULTS = 512` (`crates/storage/src/lib.rs:45`). It has no comment, no ADR, and no mention in `docs/` (grep for `512` finds nothing relevant). `MAX_PULL_EVENTS` has the same value on the line above. `audit_events` reuses the constant (`:3102`), not examined here.
- `search` clamps the caller limit to 512, filters by `m.person_id = $1`, lifecycle, supersession and scope in the `WHERE`, orders by `ts_rank_cd` descending then `memory_id` ascending, and applies `LIMIT` (`:646-705`). Person, lifecycle and filter run before the cap, so another person's rows never use a slot.
- `context` and `why_changed_context` call `search(.., usize::MAX, ..)`, which becomes 512 (`:708-718`, `:760-790`). `build_context_pack` then computes `omitted_count`, `omitted_memory_ids` and `recovery_hint` from that already-cut list (`crates/core/src/lib.rs:4341-4363`). `why_changed_context` re-sorts the 512 rows by decision reason, then relationship count, then score, before packing.
- `resume_context` and `unresolved_context` do not call `search`. They load all of the person's live memories with `all_memories_for_person` (`:3128-3149`) and pack the caller limit. No cap.
- REST routes `/search`, `/smart-search`, `/context`, `/recall`, `/why-changed` pass `request.limit` straight through (`crates/server/src/lib.rs:702-708`, `:1332-1420`). The default limit is 5 (`:2695`). MCP and CLI use the file store, not PostgreSQL.
- The file store has no such cap: `search_with_filter` sorts and truncates to the caller limit only, so file context reports omissions correctly at any size.
- The contract says that when the limit omits results the pack includes the omitted IDs, a count, and a hint to increase the limit ([`continuity-v1.md:85-96`](../contracts/continuity-v1.md)). It states no maximum limit.
- Every search, context, resume and unresolved call also loads all of the person's memories to compute contradictions (`:692`). So the 512 cap bounds the number of results built, not the rows read.

## Verdict (pre-fix)

The 512 bound itself is a plausible safety bound; its value and intent are undocumented, so this note does not call the constant a defect. What is wrong is that two callers treat the capped list as the full match set and report from it.

## Findings

### 1. Context pack under-reports omissions above 512 matches — medium, confidence high, fixed

Path: `crates/storage/src/lib.rs:708-718` into `crates/core/src/lib.rs:4346-4358`.

Trigger: a person has 513 or more live matching memories and calls `context` (REST `/context`, `/recall`) with a limit of 512 or higher, including `usize::MAX` or 513.

Impact: the pack returns 512 results with `omitted_count = 0`, no `omitted_memory_ids`, and no `recovery_hint`. The caller concludes that the answer is complete while at least one match is missing. "Increase the limit" would not help either, because the cap still applies. With a limit below 512 the count is also low by `matches - 512` (measured: 600 matches, limit 1 reported omitted 511 against a true 599; the E 600 row below shows limit 0 reporting 512 against 600), and the hint's recovery path is incomplete past the cap. `search` with limit 513 or `usize::MAX` silently returns 512 rows for the same reason.

Minimal remedy options are listed under Options.

Fixed 2026-11-13 with option 1. `search_window` adds `COUNT(*) OVER ()` to the same statement as the capped rows (`crates/storage/src/lib.rs`, `memory_search_select_sql`, `search_window`), so the total shares the person, lifecycle, supersession, filter predicate and snapshot of the rows, with no second query or copied `WHERE`. `pack_window` sets `omitted_count` to total minus returned rows, and replaces the hint with a cap-and-narrow message when `omitted_memory_ids` is shorter than the count. `search` still returns `Vec<RecallResult>` with the same 512 rows. `context` passes `usize::MAX` to `search_window`; `why_changed_context` no longer calls it (finding 2) and shares only `pack_window`. The shared-snapshot claim is for the count and the capped rows of that one statement. Resume and unresolved are unchanged. Contract text is in [`continuity-v1.md`](../contracts/continuity-v1.md) and [ADR 0006](../adr/0006-layer2-branch-aware-retrieval.md).

Check, 2026-11-13, new database per run on `cairn-pg-verify-4963`, each dropped (remaining count 0): `cargo test -p cairn-storage --test postgres -- --ignored --exact postgres_context_omissions_count_every_eligible_match_past_the_cap`. Before the fix it failed at limit 0 (`omitted_count` 512, expected 514). After: passed, 10.4s. The test seeds 513 owner matches, three foreign matches, a forgotten, an expired and a superseded row (the correction is the 514th eligible match), and checks limits 0, 1, 512, 513 and `usize::MAX`: returned rows, count equal to total minus returned, ids a bounded non-overlapping subset inside the 512 window, no ineligible ids, and a hint naming the cap without promising a larger limit. It also checks filtered counts (branch filter 516 against 514), zero matches (count 0, no hint), and, at the time of this check, why-changed limit 1 (count 513 minus 1) with an assertion that the rank-513 decision row was absent, which kept finding 2 visibly open. That why-changed part is historical: finding 2's fix replaced it, and the current test asserts the decision leads and 514 eligible matches (see finding 2's checks). Also passed: the other two ignored PostgreSQL tests, `cargo test -p cairn-storage --lib` (1 passed), `cargo fmt -p cairn-storage -- --check`, `cargo clippy -p cairn-storage --all-targets -- -D warnings`. Core and server were not rebuilt: no shared type changed.

Cost: a synthetic 50k-row probe on a mimic table (not the real schema or production latency) took about 215 ms without the window count and 411 ms with it, roughly 1.9x. An exact count still needs a scan of every match, so this is not cost-free. Skipping the count for plain `search` is a separate, unmeasured optimization and is not done.

### 2. Why-changed ranks decisions only inside the first 512 lexical matches — medium, confidence high, fixed

Path: `crates/storage/src/lib.rs`, `PostgresStore::why_changed_context` (`:777-839` after the fix; the pre-fix code was at `:760-790`).

Trigger: more than 512 matches, and a decision row (`because` or `reason:`) whose text ranks below 512 other matches. It is a realistic shape for a broad query term in a long-lived account.

Impact (before the fix): the decision-first sort never sees that row, so `why-changed` at limit 1 or 5 returns lexically stronger non-decision rows and omits the decision. The pack gives no signal that matches were cut off (same count issue as finding 1). The file store sorts all matches, so file and PostgreSQL disagree.

Fixed 2026-11-13 with choice A (the user chose ranking before the cap, not a larger window). `why_changed_context` (`crates/storage/src/lib.rs`) now runs one uncapped query that returns only `(memory_id, score)` for every eligible match, using `search_conditions`, the same person, query, forgotten, expiry, supersession and filter predicate as `search`, in `search` order. Each id is resolved in the person's live memories that recall already loads, ranked with `why_changed_priority` (`crates/core/src/lib.rs`), which calls the same `extract_decision_reason` and `code_relationships` that fill `RecallResult`, stably sorted by decision, then relationship count, then the query's lexical order, and cut to 512 before full results are built. The omission total is the number of matches that resolved in the live-memory read, so for a store with no concurrent writes count and hint are exact. The id query and the live-memory read are two separate autocommit statements with no shared transaction, so they do not share one snapshot. A match that becomes ineligible (forgotten, expired, superseded or deleted) between the two reads is dropped and not counted, and a row created in between is not seen. This cannot surface an ineligible row. If a correction supersedes a match in that gap, the pack can briefly show neither row, for one call. No single-snapshot or exact-under-concurrency guarantee is made; a transaction was not added. The finding 1 `COUNT(*) OVER ()` is a different case: it shares one statement and snapshot with its rows. No SQL `because` approximation was added, and no schema, dependency or API field changed. The core `why_changed` comparators were not touched; `why_changed_priority_matches_the_recall_result_fields` guards that the key equals the `RecallResult` fields.

Cost: work grows with the number of matches, not with 512. Every match is scored in SQL, returned as an id and a score, and inspected in Rust (including building its relationship list). Result building and contradiction scanning stay bounded by 512 results. This is not a 512-row database bound, and it was not measured at scale. Plain `search` and `context` are unchanged and keep the 512 lexical window.

Checks, 2026-11-13, a new database per run on `cairn-pg-verify-4963`, each dropped (the leftover-database count printed 0 after each earlier run): the extended `postgres_context_omissions_count_every_eligible_match_past_the_cap` first failed (rank-513 decision not first at limit 1, memory id mismatch) and then passed. It seeds 513 `whyq` matches with the decision at lexical rank 513, plus forgotten, expired, superseded and foreign decisions that score higher, and checks limits 0, 1, 5, 512, 513 and `usize::MAX` for rows returned, omitted count (514 eligible), 512 minus returned ids, the decision leading, no hidden marker or id, and that plain `search` still returns 512 lexical rows without the decision. New `postgres_why_changed_orders_like_the_file_store` compares content order with the file store's `why_changed_context` for decision groups by relationship count and ties, and limits 0, 1, 5. Also passed: the two other ignored PostgreSQL tests, `cargo test -p cairn-storage --lib` (1 passed), `cargo test -p cairn-core --lib` (40 passed), `cargo fmt --all -- --check`, `cargo clippy -p cairn-storage -p cairn-core --all-targets -- -D warnings`, `cargo check -p cairn-server`. The server check and clippy were rerun after a full disk interrupted one run. Not run: server tests, smoke scripts, HTTP routes, and a scale measurement. Docker was not reachable at the end, so a final leftover-database count was not rechecked; the 0 counts above are from the earlier runs only.

Independent rerun, 2026-11-13, after the existing Colima default profile and the existing container `cairn-pg-verify-4963` were started (no new profile, image, container, port or config change): one new database per test, `cargo test -p cairn-storage --test postgres -- --ignored --exact <name>` with the connection URL kept out of output. `postgres_context_omissions_count_every_eligible_match_past_the_cap` passed (1 passed, 77.88s). `postgres_why_changed_orders_like_the_file_store` passed (1 passed, 1.12s). Both new databases were dropped; the count of databases named `cairn_audit011%` is 0 and the server's database total is 19, the same as before the run. Server tests, smoke scripts, HTTP routes and a scale measurement were still not run. The two-read caveat above is unchanged: these tests have no concurrent writer, so they do not exercise it.

### Not defects (checked before the fix)

- Counts 511 and 512 are returned complete, with a correct `omitted_count` for limits below the count.
- Limit 0 returns nothing and reports all matches omitted, up to 512.
- Resume and unresolved over 513 session summaries are not capped: limit 513 or `usize::MAX` returns all 513, limit 512 reports 1 omitted.
- Equal-score rows are ordered by `memory_id` ascending, so the cut is repeatable, and the omitted row is the largest id.
- Another person's rows do not consume slots: an account with 600 matches did not change an account with 511.

## Evidence (pre-fix probe)

Run 2026-11-13 on container `cairn-pg-verify-4963` (`postgres:16-alpine`, localhost:5432). A new database `cairn_cap512_20261113_t09ed91` was created by `psql`, migrated by `PostgresStore::connect`, and dropped afterwards; the count of databases named `cairn_cap512%` after the drop was 0. No other database was changed. The password was not printed. The probe crate is `scratchpad/cap512` (outside the repo), built with `cargo build --offline` against this working tree; it called `bootstrap_account`, `register_account`, `remember_with_kind`, `search`, `context`, `why_changed_context`, `resume_context` and `unresolved_context`. The only direct SQL was one `UPDATE` setting `unresolved_work` on the test rows, plus read-only `SELECT` checks of rank and counts. Seed time 10s, probe time 23s.

Matching rows per account (all `capq`, one rank within a scenario unless noted): A 511, B 512, C 513, D 513, E 600, F 513 session summaries.

| Scenario | Result |
| --- | --- |
| A 511: `search` limits 0/1/511/512/513/MAX | 0/1/511/511/511/511. `context` 511, 512, 513 and MAX report omitted 0, no hint. Correct. |
| B 512: same limits | 0/1/511/512/512/512. `context` limit 511 omitted 1; 512, 513, MAX omitted 0. Correct. |
| C 513: same limits | 0/1/511/512/**512**/**512**. `context` limit 511 omitted 1. Limits 512, 513, MAX: 512 results, **omitted_count 0, no ids, no hint**, while 513 rows match (one match is missing with no signal). Finding 1. |
| C 513 `context` limit 0 and 1 | Omitted 512 and 511; true values 513 and 512. |
| E 600 | Same shape as C: sMAX 512; `context` limit 0 omitted 512 (true 600). |
| C 513 ties | Two calls identical; ids ascending; the 513th inserted id `…629379000` was the one missing. First 3 of limit 3 equal the first 3 of the full list. |
| Isolation A vs E | A 511, E 512, overlap 0. |
| D 513 why-changed | SQL check: the decision row is one of 513 matches, rank 0.1 against 0.3 for the other 512. `why_changed` limit 1 and 5 returned no decision row; `search` MAX had 512 rows with none a decision; `context` limit 513 returned 512 rows, no decision, omitted 0. Finding 2. |
| F 513 session summaries | `resume` and `unresolved`: limit 5 → 5 results, omitted 508; 512 → 512, omitted 1; 513 and MAX → 513, omitted 0. No cap. |

Limitations: one database, default recall filter only, explicit memories and session summaries only. A filtered run, a project scope run and a run with contradiction-heavy rows were not made. `limit` of 512 against a database that returns exactly 512 is indistinguishable from the cap. HTTP routes were not called: the Rust store methods were called directly, and the routes pass `request.limit` through unchanged. Live ports 3000 and 62200 were not contacted. No workspace test, Clippy or format check was run, since no code changed.

## Options considered (options 1 and 4 accepted and done)

1. **Honest count, keep the cap (accepted, done 2026-11-13).** Add `COUNT(*) OVER ()` to the same capped statement, so the count shares the rows' predicate and snapshot, and set `omitted_count` to total minus returned rows. `omitted_memory_ids` stay bounded by the cap, and the hint says the list may be shorter than the count and to narrow the query or filter. Contract text is in `continuity-v1.md` and ADR 0006.
2. **Document the cap only.** State in `continuity-v1.md` that PostgreSQL ranks at most 512 matches and that `omitted_count` counts within that window. Cheapest, but finding 1 remains and finding 2 is only disclosed.
3. **Reject or document limits above 512 on `search`.** An explicit validation error instead of a silent clamp. It would break callers that send `usize::MAX`, so check callers first.
4. **Why-changed fix (choice A done 2026-11-13).** The options were (a) rank decision rows in SQL with a second bounded query, which duplicates `extract_decision_reason`; or (b) page the search past 512, which changes limit semantics. The user chose neither: rank every eligible match with the core helpers before the cut (see finding 2).

Options 1 and 4 are done. No decision remains in this note.
