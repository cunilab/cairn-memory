---
id: CM-AUDIT-009
title: "Account search limit — 2026-11-13"
status: draft
---

# Account search limit — 2026-11-13

Account search is supposed to return that person's matching memories, best first, up to the caller's limit. This note checked that order. Before the fix, the file path applied the limit and then the account boundary. It does not redesign ranking, embeddings, or lifecycle.

## Progress

| Item | Status |
| --- | --- |
| Capture findings 1–7, choice B, and device-only offline enqueue | Accepted in [CM-AUDIT-008](./2026-11-12-memory-capture.md). Not rechecked here. |
| Account and anonymous offline enqueue, old-data republishing, identity-field secrets and unsupported patterns | Still open. Not expanded. |
| Account-scoped search limit | Fixed in `search_for_account`. Findings 1 and 2 checked 2026-11-13. |
| Resume and unresolved contradiction ids | Fixed in `retain_account_recall` before the pack limit. This does not close the rest of account retrieval. |

## Fix

`retain_account_recall` keeps rows in `memory_owners` for the token's person, drops `contradiction_ids` that person does not own, and sets `contradictory` from the ids that remain (`crates/core/src/lib.rs:4131-4141`). `search_for_account` scores eligible matches with `usize::MAX`, calls that helper, then truncates to the caller limit (`:4143-4155`). `resume_for_account` and `unresolved_for_account` authenticate, scan with `usize::MAX`, call the same helper, then pack the caller limit (`:4168-4197`). Context and why-changed still call search. `decision_reason`, `relationships`, `stale`, and `stale_reason` come from the owned memory and the recall filter. Unscoped search, unscoped resume, CLI, MCP, and PostgreSQL are unchanged. No dependency, schema, or ADR change. ADR 0008 and CM-CONTRACT-008 already require this account boundary. This does not finish account retrieval.

An owned contradiction id stays even when that memory is outside this result page. This pass does not add a second eligibility pass over contradiction targets, and it does not add shared-memory search.

Checked 2026-11-13, working tree only, temporary stores created by the tests:

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib -- account_search_limits_owned_matches_and_hides_foreign_contradiction_ids -- --exact` before the fix | Failed. Limit 1 length was 0, expected 1. Exit 101. Test time 0.73s |
| Same command after the fix | Passed. Test time 0.65s |
| `cargo test -p cairn-core --lib` | 37 passed, 0 failed, 2.08s. Includes the new test, `account_sessions_isolate_context_for_one_public_id`, `scoped_recall_keeps_branch_memory_isolated`, and `opening_a_store_sweeps_expired_records_into_tombstones` |
| `cargo test -p cairn-server --lib -- file_http_capture_uses_file_bearer_when_accounts_exist -- --exact` | 1 passed, 0.62s |
| `cargo fmt -p cairn-core -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |

### Resume contradiction followup

Search was accepted with resume and unresolved still attaching contradiction ids from the whole file. A same-public-session handoff with opposite negation leaked the other summary id. The earlier resume sentence in finding 1 used `ownerfact` and `memberfact`. Those texts do not contradict, so that pack did not prove these ids were sanitized.

`account_resume_and_unresolved_hide_foreign_contradiction_ids` failed before the helper: the owner handoff row was `contradictory`. After `retain_account_recall`, the owner resume and unresolved JSON omit the other person's id, summary id, event id, and `foreignmarker`. An owned pair on one branch stays `contradictory`, including when the other owned id is only in `omitted_memory_ids`. Limit 0 still returns no rows. Unscoped `resume_context` still contains the foreign id.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib -- account_resume_and_unresolved_hide_foreign_contradiction_ids -- --exact` before the helper | Failed. `assert!(!handoff.contradictory)`. Exit 101. Test time 0.59s |
| Same command after the helper | Passed. Test time 0.59s |
| `cargo test -p cairn-core --lib` | 38 passed, 0 failed, 2.41s. Includes the search test and this handoff test |
| `cargo test -p cairn-server --lib -- file_http_capture_uses_file_bearer_when_accounts_exist` | 1 passed, 0.64s. That test now ends `handoff-topic` for two accounts and checks file HTTP resume and unresolved, including limit 0 |
| `cargo test -p cairn-server --lib -- resume_and_unresolved_routes_return_structured_handoff` | 1 passed, 0.00s. Unscoped route, no accounts |
| `cargo fmt -p cairn-core -p cairn-server -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |

## Path

The rest of this note is the pre-fix reading. File `search_with_filter` keeps memories that are not forgotten, expired, or superseded, then applies the scope filter, scores by term count, sorts by score descending and `memory_id` ascending, and truncates to `limit` (`crates/core/src/lib.rs:4075-4114`, `:4321-4325`, `:5687-5717`, `:5728-5749`). Before the fix, `search_for_account` called that with the caller limit and only then kept rows in `memory_owners` for the token's person (`:2686-2691`).

`context_for_account` and `why_changed_for_account` call `search_for_account` with `usize::MAX`, then `build_context_pack` applies the caller limit (`:4146-4155`, `:4193-4218`, `:4339-4360`). `resume_for_account` and `unresolved_for_account` collect with `usize::MAX`, drop other owners, then pack (`:4157-4190`). Those four do not use the caller limit as a global cutoff.

File HTTP `POST /cairn-memory/search` passes `request.limit` into `search_for_account` when `file_bearer` returns a token (`crates/server/src/lib.rs:2251-2273`). Once any account exists, a missing bearer is 401 (`:2622-2630`). A store with no accounts still uses unscoped `search_with_filter`. The default limit is 5 (`:2695-2697`). Limit 0 is accepted and truncates to nothing.

`cairn search` and MCP `memory_search` / `memory_smart_search` call `search_with_filter` (`crates/core/src/bin/cairn.rs:130-138`, `crates/server/src/mcp.rs:249-255`). That unscoped local path is intentional. MCP rejects a limit below 1 (`:1037-1045`). This limit bug does not apply to them.

PostgreSQL `search` puts `m.person_id = $1` in the `WHERE`, then orders by `ts_rank_cd` descending and `memory_id` ascending, then `LIMIT` (`crates/storage/src/lib.rs:646-705`). The same function clamps the limit to 512 (`:45`, `:659`). `context` and `why_changed_context` call `search` with `usize::MAX`, so they see at most 512 of that person's matches before the pack limit (`:708-718`, `:760-768`). File context has no 512 cap. That cap does not drop another person's rows. HTTP PostgreSQL search passes `request.limit` into this function (`crates/server/src/lib.rs:1332-1347`).

Branch, worktree, project, and session filters run inside `search_with_filter` before its truncate. Ownership does not. An expired row is dropped before the truncate. A live foreign row that matches the filter still occupies a slot.

## Findings

### 1. File account search applies the limit before ownership — high, confidence high, fixed

Paths: `crates/core/src/lib.rs:4131-4144` and `:4113`. HTTP caller: `crates/server/src/lib.rs:2258-2266`.

`search_for_account` asks for the caller's N best rows in the whole file, then discards every row the account does not own. A foreign or unowned row with a higher term count uses up that N. The response does not include the discarded content. The account's own matches that sat past N are missing. Equal scores use `memory_id` ascending, so a foreign row with the smaller id can take the last slot the same way.

Trigger: two accounts and one unowned explicit memory. The query is one shared token. The foreign text repeats it three times, the unowned text twice, and each owned text once. Account search then sees the global order foreign, unowned, owned, owned.

Impact: limit 1 and limit 2 return `[]`. Limit 3 returns only the earlier owned id. Limit 4 returns both owned ids, score 1 then score 1, in `memory_id` order, and a second call matches. None of those bodies contain the foreign or unowned marker or id. Unscoped `search_with_filter` limit 1 returns the foreign id. That is the global ranking the CLI uses, not a second auth bug. Limit 0 returns `[]` on this path and on PostgreSQL. That is truncate-to-zero, not the ownership bug.

The same cutoff hides an equal-score owned row when the foreign id is smaller: limit 1 is `[]` and limit 2 returns the owned id, with no foreign text. When the owned id is smaller, limit 1 returns that owned row.

A branch filter that does not match the foreign row returns the owned global row at limit 1. The same query filtered to the foreign branch returns `[]` at limit 1. An expired foreign row with the higher count does not occupy the slot; limit 1 returns the live owned row.

Context and why-changed do not have this false empty. Context limit 1 returned the earlier owned row and its omitted id was the other owned row, not the foreign id. Resume limit 1 returned `memory-{person_id}-handoff-session` for the caller. That fixture was `ownerfact` and `memberfact`, which do not contradict, so the pack's lack of the other summary id did not prove contradiction ids were sanitized. Why-changed limit 1 matched context.

PostgreSQL does not have this false empty. On a new database, the member document scored 0.4, the owner's stronger document 0.2, and the owner's weaker document 0.1. A global `LIMIT 1` without the person predicate returned the member id. Account search limit 1 returned the 0.2 owned id. Limit 2 returned both owned ids in that score order. The owner body did not contain the member marker or id. Context limit 1 omitted only the other owned id.

Check, file, on a new temporary store, then deleted: `CARGO_TARGET_DIR=/tmp/cairn-limit-target cargo run --quiet --manifest-path /tmp/cairn-account-limit-repro/Cargo.toml`. Exit 0. Limits 1 and 2 were empty; limit 3 was the first owned id; limit 4 was both owned ids.

Check, PostgreSQL, container `cairn-pg-verify-4963`, database `cairn_limit_20261113_task6c56527`, created for this run and dropped after: `CARGO_TARGET_DIR` was this repo's `target`, manifest `/tmp/cairn-pg-limit-repro/Cargo.toml`. Exit 0. No other database was changed. The password was not printed.

Existing `account_sessions_isolate_context_for_one_public_id` still passes and does not cover this. It searches `ownerfact` with limit 5 (`crates/core/src/lib.rs:6276-6287`). The other account's text is `memberfact`, so it cannot fill the limit. Command: `cargo test -p cairn-core --lib -- account_sessions_isolate_context_for_one_public_id -- --exact`. Result: 1 passed, 0.92s. The PostgreSQL tests around `crates/storage/tests/postgres.rs:336-344` and `:812-828` use the same shape. They were read, not run.

Minimal fix, one function: in `search_for_account`, call `search_with_filter` with `usize::MAX`, retain owned rows, then `truncate(limit)`. That is the same order `resume_for_account` already uses. `context_for_account` and `why_changed_for_account` pass `usize::MAX`, so they still pack the caller's limit afterward. No CLI, MCP, or PostgreSQL change. No new dependency. No ADR: ADR 0008 and CM-CONTRACT-008 already require the account boundary. This restores it. Add an ADR only if a later change alters the retrieval model.

Fixed 2026-11-13 by the function change in Fix. The test is `account_search_limits_owned_matches_and_hides_foreign_contradiction_ids`. PostgreSQL does not need a migration.

### 2. A returned owned row can name another person's memory id — medium, confidence high, fixed

Path: `contradiction_ids` scans every memory (`crates/core/src/lib.rs:5507-5529`). `search_with_filter` passes `self.state.memories` into `recall_result` (`:4092-4101`, `:5307-5336`). PostgreSQL passes `all_memories_for_person` (`crates/storage/src/lib.rs:692-703`), so this is file-only.

Trigger: both rows are global. The owned text is `quotadelta widget does not apply`. The foreign text is `quotadelta widget does apply`. They share three topic tokens and only the owned text has `does not `.

Impact: account search and `context_for_account` return the owned row, and `contradiction_ids` contains the foreign `memory_id`. The foreign sentence is not in the JSON. Context omitted ids do not include it. This is an identifier side channel, not the missing-result bug in finding 1. Read, not called: `correct_for_account` (`:3844-3858`) goes through `reject_other_owner` (`:2730-2746`). A foreign owner that is not a share returns `unknown memory or event: {target_id}` and does not return the stored memory. This pass did not call that function or walk every other id route.

The limit fix alone does not remove these ids, because `recall_result` runs before the owner retain. Fixed 2026-11-13: `retain_account_recall` drops contradiction ids the account does not own and sets `contradictory` from what remains. Search, resume, and unresolved call it before the caller limit. Unscoped search and unscoped resume still return the foreign id. The search test covers an owned-to-owned contradiction. The resume test covers a contradictory same-public-session handoff.

Tied scores did not put foreign text or a foreign id into the search body. They only changed which owned row survived finding 1.

## Not checked

Workspace tests, server Clippy, smoke scripts, and the ignored PostgreSQL suite were not run for this followup. Live ports 3000 and 62200 were not contacted. No dependency, schema, commit, push, or deploy changed. The 512-row PostgreSQL context cap was read, not filled. Share-grant search was not reviewed. A Global owned row combined with a branch filter was not reviewed. Resume and unresolved contradiction ids are checked for the handoff fixture above, not for every account route. Remote AgentMemory was not called, and no local memory daemon was started. The original temporary file store was already removed. Database `cairn_limit_20261113_task6c56527` was already dropped.
