---
id: CM-AUDIT-010
title: "Recall filter and contradiction targets — 2026-11-13"
status: draft
---

# Recall filter and contradiction targets — 2026-11-13

This note answers two selection questions left open by [CM-AUDIT-009](./2026-11-13-account-search-limit.md). It does not change ranking, embeddings, share-grant search, or the PostgreSQL 512 cap.

## Progress

| Item | Status |
| --- | --- |
| Capture findings 1–7, choice B, privacy, and device-only offline enqueue | Accepted in [CM-AUDIT-008](./2026-11-12-memory-capture.md). Not rechecked. |
| Account search limit and owned-versus-foreign contradiction ids | Accepted. Same-public-session account resume still omits the other person. |
| Global memory under a branch, worktree, or project filter | Intentional inheritance. Not a defect. |
| Contradiction targets that are forgotten, expired, superseded, or outside the filter scope | Choice B implemented. A target must be eligible for recall. Mismatched scope stays rejected. No automatic winner. |
| Share-grant search and the PostgreSQL 512 cap | Still queued. Not reviewed. |

## What the contracts say

[CM-CONTRACT-006](../../contracts/continuity-v1.md) matches `global` on any scoped query so personal guidance stays available, and it keeps branch and worktree rows narrow. [CM-ADR-0006](../../adr/0006-layer2-branch-aware-retrieval.md) says an explicit memory with no repository is user-global. A project fact must not be promoted into global memory ([CM-SPEC-002](../../specs/memory-context-and-promotion.md)); `remember` without a repository still stores `global` by `memory_scope`.

A contradiction signal is same-scope text with opposite negation and at least two shared topic terms ([CM-CONTRACT-006](../../contracts/continuity-v1.md)). [CM-CONTRACT-004](../contracts/lifecycle-v1.md) and [CM-ADR-0010](../../adr/0010-correction-keeps-linked-history.md) exclude expired, forgotten, and superseded rows from normal recall. The predecessor stays in storage and export through `supersedes`. The signal has no separate provenance field: `contradictory` is true exactly when `contradiction_ids` is non-empty.

## Filter behavior

`memory_matches_filter` (`crates/core/src/lib.rs:5694-5725`) and PostgreSQL `append_filter_conditions` (`crates/storage/src/lib.rs:4004-4043`) use the same ladder:

| Stored scope | Matches a filter when |
| --- | --- |
| `global` | Any session or context filter is present, including another repository |
| `project` | The filter has that repository |
| `branch` | The filter has that repository and branch |
| `worktree` | The filter has that repository, branch, and worktree |
| `session` | The filter has that `sessionId` |

A narrower filter still returns broader owned rows. A branch filter returns matching branch rows, that repository's project rows, and global rows. A worktree filter also returns the branch row. A repository-only filter does not return branch or worktree rows. A branch with no repository does not return project, branch, or worktree rows. A session summary with no repository does not match a branch filter. Another repository's project, branch, and worktree rows stay out.

That is inheritance of broader personal and project guidance, not cross-project contamination of a scoped row. `scoped_recall_keeps_branch_memory_isolated` already expects the global row in both the `main` and branch results.

## Contradiction targets

Result rows use `memory_is_recalled` (`crates/core/src/lib.rs:4312-4332`): not forgotten by memory id or source event id, not expired, and not superseded by a live successor. `contradiction_ids` (`:5514-5536`) scans every memory, skips only a forgotten memory id, and does not consult the recall filter. `same_scope_context` (`:5539-5557`) is what blocks a different branch, worktree, repository, or scope. `retain_account_recall` (`:4131-4140`) then drops ids that person does not own. It does not apply lifecycle eligibility.

PostgreSQL search, resume, and unresolved call `recall_result` with `all_memories_for_person` (`crates/storage/src/lib.rs:3128-3141`, `:692-703`). That query already drops forgotten ids, source-event tombstones, expired rows, and superseded rows, and it passes an empty forgotten set. The file store passes `self.state.memories`.

## Checks

No application code, dependency, commit, push, or live port was changed. The file harness is `/tmp/cairn-audit-010-harness` and deletes its store. The PostgreSQL harness is `/tmp/cairn-audit-010-pg`. It created `cairn_audit010_20261113` on container `cairn-pg-verify-4963` and dropped only that database. Existing databases on that container were left in place.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib -- scoped_recall_keeps_branch_memory_isolated -- --exact` | Passed. 0.01s. 37 other lib tests filtered out. |
| `/tmp/cairn-audit-010-harness` | Passed. 0 failures. |
| `/tmp/cairn-audit-010-pg` on the disposable database | Passed. Database absent after the run. |

The file harness used one owner and one other account. Global, project, branch, and worktree rows for `alpha-repo` and `beta-repo` confirmed the ladder above. A live global pair stayed contradictory, and the account result named only the owned peer. Unscoped search still named the other person's id. Limit 1 returned one row and kept the owned peer id. Limit 0 returned no rows.

A superseded global pair returned only the correction. Its search, context, and why-changed results had `contradictory` true and `contradiction_ids` equal to the predecessor. Export still stored that predecessor. Reopen did not clear the id.

An already-expired global peer was absent as a result and present as the only contradiction id. After `MemoryStore::open` swept it into `forgotten_ids`, the id was gone and `contradictory` was false.

`forget_for_account` on a memory id removed that id. `forget_for_account` on a session observation id hid the worktree summary from search and left that summary id on the live explicit row in search, context, and why-changed. The hidden summary text was not in the returned body.

The same public session id produced two session-scoped summaries. Account resume and unresolved returned the owner row with an empty contradiction list. Unscoped resume still named the other person. A branch filter did not return the session-scoped summary.

A different branch, worktree, or scope was not cited. Same-branch and same-worktree peers were cited.

PostgreSQL returned the global rows under a `beta-repo` worktree filter and omitted the alpha project and branch rows. The correction, the expired peer, and the forgotten memory id did not set `contradictory`. Search, context, and why-changed were empty for the superseded id. A live global pair still contradicted. The source-event forget path was not executed on PostgreSQL. `all_memories_for_person` excludes a forgotten source event id in SQL, so that path does not have the file behavior.

## Findings

1. **File recall names an owned summary forgotten by its source event.** Status: fixed 2026-11-13 by choice B. Severity medium. Confidence high. `memory_is_forgotten` treats a forgotten source event as forgotten (`:4312-4318`). `contradiction_ids` checks only `forgotten_ids.contains(memory_id)` (`:5527`). Account search, context, and why-changed therefore set `contradictory` from a row that recall will not return. Forgetting the memory id itself does not do this. PostgreSQL does not, by the `all_memories_for_person` predicate. This is the same class of identifier side channel CM-AUDIT-009 closed for foreign ids, now for an owned tombstone.

2. **File recall treats a superseded predecessor as an active competitor. PostgreSQL does not.** Status: fixed 2026-11-13 by choice B. Severity medium. Confidence high for the behavior. The file correction is the only result, and its contradiction id is the row `memory_is_recalled` skipped. ADR 0010 says normal recall returns the correction and keeps the old row in storage and export. The contract does not say whether that old id belongs on the active flag. Two choices:
   - **A.** Keep the id as historical provenance and accept `contradictory: true`. PostgreSQL would then need to put superseded rows back into the contradiction scan. The flag would mean "this text once conflicted," including with the memory it replaced.
   - **B, recommended.** `contradiction_ids` uses the same eligibility as result rows. `supersedes` and export keep the history. The file store would match the PostgreSQL result from this run. The flag would mean "a recallable same-scope memory still competes."

3. **File recall names an expired peer until the next sweep.** Status: fixed 2026-11-13 by choice B. The flag clears without reopening the store. Severity low. Confidence high. `sweep_expired` (`:981-1013`) runs on open and import, not on search. Until then the expired row is hidden as a result and still sets the flag. After open, the memory id is tombstoned and the flag clears. PostgreSQL excludes expired rows in the scan, confirmed by the disposable run. A long-lived file process keeps finding 3 until restart.

Mismatched scope is not a finding. `same_scope_context` rejected the other branch, the other worktree, and the other scope in the harness. Global inclusion is not a finding either.

## Fix

Choice B is implemented. `contradiction_ids` skips a target forgotten by memory id or source event, expired at recall time, or superseded by a live successor. Those checks are `memory_forgotten`, `is_expired`, and `live_superseded_target`, the same rules `memory_is_recalled` uses. The successor id set is built from the slice passed to `recall_result`. File search passes full history. PostgreSQL passes `all_memories_for_person`, which already drops those rows, so the extra filter does not need full history and does not change that query. `supersedes` and export still keep the predecessor. `retain_account_recall` is unchanged. There is no winner and no new response field.

The test is `contradiction_targets_follow_recall_eligibility`. It does not reopen the store. An owned live pair stays active, including at limit 1 when the peer is only in `contradiction_ids`. A different branch is not cited. A foreign id remains on unscoped search and is still removed for the account. A live correction is the competitor until its `expires_at` is already past; the predecessor then competes and the correction does not. Export still contains the predecessor, the expired peer, the forgotten memory, the source-event-forgotten summary, and the forgotten successor. Forgetting the successor memory id makes the predecessor eligible again. Forgetting a memory id directly still leaves that row stored and inactive.

Checked 2026-11-13, working tree only, temporary stores created by the tests. PostgreSQL was not rerun. The shared helper cannot cite a predecessor that `all_memories_for_person` already omitted, and it does not drop a live correction merely because `supersedes` is set. `RecallResult` did not change.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib -- contradiction_targets_follow_recall_eligibility -- --exact` before the fix | Failed. A live peer's `contradiction_ids` contained the predecessor and the correction. Exit 101. Test time 0.60s |
| `cargo test -p cairn-core --lib` | 39 passed, 0 failed, 7.58s |
| `cargo test -p cairn-server --lib -- why_changed_route_returns_reason_relationships_and_conflicts` | 1 passed, 0.01s. 28 other lib tests filtered out |
| `cargo test -p cairn-server --lib -- correction_replaces_recall_without_destroying_provenance -- --exact` | 1 passed, 0.00s. 28 other lib tests filtered out |
| `cargo fmt -p cairn-core -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |

## Chosen correction

Choice B was selected and implemented in the Fix section. The change is `contradiction_ids`, so every `recall_result` caller inherits it: `search_with_filter`, `resume_context`, `unresolved_context`, and the account search, context, resume, unresolved, and why-changed paths. `retain_account_recall` stays the ownership pass. PostgreSQL already filters the slice; there is no migration. `project_conflicts` passes `live_project_memories` into `competing_project_memories`, so it stays outside this gap. Choice A was not taken. Ranking cap, share-grant search, and a second eligibility pass over foreign ids stay out of this change.

## Limits

The paragraphs above this Fix section describe the investigation, not the current code. Workspace tests, smoke, share-grant search, the 512 cap, and live Next.js or API ports were not run for the fix. PostgreSQL was not rerun; source-event forget on PostgreSQL remains read from SQL. A stored `global` row that also carries a repository was not imported. The new file test covers search, context, why-changed, resume, and unresolved. It does not open a second process.
