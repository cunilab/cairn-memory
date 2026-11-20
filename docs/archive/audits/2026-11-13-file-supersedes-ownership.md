---
id: CM-AUDIT-012
title: "File store supersedes ownership — 2026-11-13"
status: draft
---

# File store supersedes ownership — 2026-11-13

This note checks one residual left open by [CM-AUDIT-011](./2026-11-13-postgres-search-cap.md): in the file store, whether a `supersedes` link written by one account can hide another account's memory. Read-only audit: no application, dependency, schema, or lock change. It does not review lifecycle, sharing, or the rest of import.

## Progress

| Item | Status |
| --- | --- |
| Count and why-changed ranking (CM-AUDIT-011) | Closed in that note. Not rechecked. |
| Foreign `supersedes` in the file store | This note. Finding 1 reproduced over HTTP, fixed 2026-11-13 with option 1 (stored inert), independent review accepted. |
| Foreign `forgottenIds` on account import | **Open** (finding 2, reproduced). Not fixed by this change. Queued as its own validation and fix. |
| Project views and import `project_id` | **Unverified, queued.** A source-read concern from the independent review, not probed and not part of this fix. See Limits. |
| Broad lifecycle and retention, sharing, code-quality, docs, and unused-code audits | Still queued. Not reviewed. |

## Path (pre-fix reading)

This section and the Trigger, Impact and reproduction of finding 1 describe the code before the fix. Line numbers are from then. The fix is under Fixed.

- `import_for_account` (`crates/core/src/lib.rs:3937-4000`) rejects an imported memory or observation whose own id belongs to another person (`:3945-3963`). It does not look at `supersedes`. After the merge it records the caller as owner of every imported memory id (`:3991-3993`). `normalize_export_bundle` (`:4743-4800`) validates ids, size, redaction and lifecycle, and nothing about `supersedes`.
- The HTTP route `/cairn-memory/import` (`crates/server/src/lib.rs:2475-2489`) passes the request bundle to `import_for_account` when the caller has a bearer. Any registered account can call it.
- `memory_is_superseded` (`:4316-4321`) scans the whole file for any live successor, through `live_superseded_target` (`:5527-5535`), which checks only forgotten and expired. It does not compare owners. `memory_is_recalled` (`:4323-4327`) uses it, so `search_with_filter` (`:4075-4112`) drops the target before `retain_account_recall` (`:4131-4140`) runs. Resume and unresolved use the same check.
- `contradiction_ids` (`:5537-5565`) builds its skip set from the same helper over the slice it is given. The file store passes the whole file, so a foreign successor also removes the target from every other row's contradiction ids.
- `correct_for_account` (`:3844-3857`) rejects a target the caller does not own (`reject_member_mutation`). The unscoped `correct`, used by the local CLI and MCP, creates an unowned successor on purpose. Both are trusted local paths.
- PostgreSQL requires the same person: `not_superseded_by_live_memory_sql` (`crates/storage/src/lib.rs:3946-3975`) has `newer.person_id = m.person_id`, and the search, context, resume and contradiction slices are all that person's rows. `insert_memory` binds the authenticated person, not the bundle (`:3722-3750`). Not run on a database; the source is decisive for the successor rule.
- Project paths are not affected by this finding: `live_project_memories` (`:2903-2921`) and the project detail list (`:1577-1592`) count a successor only when it has the same `project_id`. Not probed.

## Findings

### 1. A foreign `supersedes` link hides another account's memory — medium, confidence high, fixed

Path: `crates/core/src/lib.rs:4316-4321`, `:5527-5565`, reached from `:3937-4000` and `crates/server/src/lib.rs:2475`.

Trigger: account B imports a bundle with one B-owned memory whose `supersedes` is the id of an account A memory. B must know the id. Ids are `memory-explicit-<time>` or `memory-person-<time>-<session>`, and a shared project memory carries `promoted_from` (the source id) to team members.

Impact:

- A's search, context and resume no longer return the target, and A has no signal: A's export still holds the row.
- Contradiction ids on A's other rows lose the target, so `contradictory` flips to false.
- B's row is never shown to A, and no content of A reaches B (B's search for A's words returns nothing).
- Only B can undo it, by forgetting or expiring B's row. A cannot remove it. This is cross-account availability and integrity loss with no disclosure.

Reproduction, 2026-11-13: the real `cairn-server` on a temporary port with a new store file, accounts A and B through HTTP, probe `/tmp/cm-audit012-supersedes-probe.py`. A had two contradicting memories.

| Step | Result |
| --- | --- |
| Baseline A context | both rows, each `contradictory` with the other |
| B imports one row `supersedes` = A's second row | HTTP 200, 1 memory imported |
| A context and search | the target is gone; the first row is `contradictory: false` |
| A export | still contains the target row |
| B context and search | B sees its own row; no A content appears |
| B forgets its own row | A's context returns both rows, contradictions back |
| B imports an expired or new row again | the target disappears again |
| B imports `supersedes` = an id that does not exist | HTTP 200, stored |
| A session summary, B row `supersedes` it | A's `/resume` returned the summary before and an empty list after |

Not exercised: unscoped CLI and MCP (trusted by design), project conflicts, a PostgreSQL run.

Fixed 2026-11-13 with option 1, as approved by the user (stored inert, no import rejection). The rule is in `superseded_targets` (`crates/core/src/lib.rs`): a live successor hides its target unless the successor has an owner and the target's owner is not that same owner. So a successor with no owner counts, an owned successor of an unowned or unknown target does not, and a successor owned by the target's owner counts. It decides both normal-recall eligibility and the contradiction skip set, so the target and the rows that cite it stay consistent. `MemoryStore::live_superseded_targets` builds the owner map (first record wins, like `item_owner`) and the set once per request, and `search_with_filter`, `resume_context`, `unresolved_context` and shared project memories use it. That also removes the earlier scan of the whole file per memory and per result. `recall_result` keeps its public signature: it computes the set from its slice with no owners (PostgreSQL passes one person's rows) and calls a private `recall_result_with`; `competing_project_memories` builds the set the same way. The link, the export and import behavior, `correct_for_account` and the PostgreSQL query are unchanged. Nothing is rejected at import and no field was added. Previously stored foreign links stop having an effect at once, and stay inert if their target is created or restored later.

Checks, 2026-11-13: the three core tests below were written first and run with the owner rule disabled (one line, restored afterwards). All three failed, then passed with the fix. The HTTP test failed the same way and passed.

- `foreign_supersedes_link_is_stored_but_does_not_change_account_recall`: the other account imports links to the owner's memory and session summary. The owner's search, context and resume are unchanged, contradiction flags stay, the other account gets only its own rows and no owner content, the owner's export keeps the target row, and the other account's export keeps the link unchanged.
- `same_owner_and_unowned_successors_still_hide_their_target`: an owner correction hides its target and forgetting the correction revives the target; a local unscoped correction of an owned memory still hides it; an owned link to an unowned memory stays inert.
- `foreign_link_to_unknown_target_stays_inert_after_the_target_is_restored`: an import with an unknown target is stored unchanged and the target, restored later by its owner, is recalled.
- `foreign_supersedes_import_does_not_hide_another_accounts_memory_over_http` (server crate): the same flow through `/import`, `/search`, `/context`, `/resume` and `/export` with two bearers on the router.

Also passed: `cargo test -p cairn-core --lib` (43), `cargo test -p cairn-server --lib` (30), `cargo fmt --all -- --check`, `cargo clippy -p cairn-core -p cairn-storage -p cairn-server --all-targets -- -D warnings`, and the ignored PostgreSQL test `postgres_why_changed_orders_like_the_file_store` on a new database that was then dropped (the `cairn_audit012%` count is 0; 19 databases before and after). Forgotten and expired successor revival is covered by the existing `contradiction_targets_follow_recall_eligibility` and the account quota tests in that suite, which passed unchanged. Not run: the other PostgreSQL tests, smoke scripts, a live server, a scale measurement. Independent review of this change: accepted (`/tmp/cm-audit012-finding1-review.md`). It re-ran core 43, server 30, fmt and clippy, and re-ran the four tests with the rule disabled on a copy: all four failed. [ADR 0010](../../adr/0010-correction-keeps-linked-history.md) went to `proposed` for this edit, as [the documentation standards](../../standards.md) require, and returned to `accepted` on 2026-11-13 after the user-approved option and this accepted review.

### 2. Adjacent: account import accepts foreign `forgottenIds` — confidence high, OPEN, not fixed

`import_for_account` checks no ownership of `bundle.forgotten_ids`. In the same probe, B imported `forgottenIds: [A's memory id]` and A's context returned nothing afterwards (HTTP 200, `imported_forgotten_ids: 1`). That is the same trust gap with a stronger effect, and it needs its own audit item and contract decision. It is listed here only so it is not lost.

## Options

1. **Stored inert (recommended).** Keep accepting and storing the link. A successor counts only when its owner is not a different person from the target's owner. A successor with no owner (local CLI and MCP, or rows from before accounts) still counts, so the trusted local correction keeps working.
2. **Reject at import.** `import_for_account` refuses a memory whose `supersedes` names a row owned by another person, like the existing id check. Unknown targets stay allowed.

Recommendation: option 1, because:

- It is the only complete one. A target can start to exist after the link was stored (the owner restores a partial backup, or the link was stored before this change). Rejecting at import cannot see that.
- It does not reject legitimate exports. An export of one account holds only that account's rows, and a backup with an unknown target is valid and stays valid.
- It keeps the export contract: provenance links are stored unchanged ([export-v1](../contracts/export-v1.md)). No new field, no winner.
- It cannot affect PostgreSQL, which already has the rule.

Option 2 can be added later as a clearer error, but it is not needed. Rejecting also changes an import behavior, which needs a contract line. Option 1 was approved and is the one implemented.

## Remedy scope as planned (historical, pre-fix)

This was the plan before the fix. It names the deleted `memory_is_superseded` and old line numbers. The current code is the Fixed paragraph under finding 1.

- One owner rule, used in two places in `crates/core/src/lib.rs`. A link is foreign when the successor has an owner and it differs from the target's owner. `memory_is_superseded` ignores a foreign link. The contradiction skip set (`:5537-5565`) ignores it too, otherwise the target would show in results while the other rows stop citing it. `recall_result` is public and PostgreSQL and tests call it, so the file store should pass the owner-aware skip set in through a private inner function, leaving the public signature as it is.
- Build the owner lookup once per request as a map. `memory_owners` is a list, and a per-memory list scan inside the existing per-memory scan would add a third factor of n.
- Callers: `search_with_filter`, `resume_context`, `unresolved_context` (and so the `_for_account` and why-changed paths). As built, shared project memories and `competing_project_memories` use the same helper too. Local unscoped calls on a store with no owners behave as today.
- Tests to add: B's foreign link leaves A's target and contradictions unchanged; a same-owner correction still supersedes; an unowned successor still supersedes (CLI and MCP); a link to an unknown id is harmless; a foreign link stays inert after A later creates or restores the target.
- Docs: a line in [continuity-v1](../../contracts/continuity-v1.md) that a successor must belong to the same person, and a line in [ADR 0010](../../adr/0010-correction-keeps-linked-history.md). The export contract needs no change. No migration.

## Evidence

- Source reads at the lines above, 2026-11-13, working tree with uncommitted changes.
- Probe run: `python3 /tmp/cm-audit012-supersedes-probe.py` against `target/debug/cairn-server` on 127.0.0.1:62741 with a store file in the session scratch directory, then the process was stopped. Ports 3000 and 62200 and the user stores were not used.
- `cargo build -p cairn-server` finished before the probe. The probe ran before the fix, and it made no repo source change. The fix checks are under Fixed.

## Limits

- A single run of the HTTP probe. The order of the owner list was not varied.
- Id guessing was not tested. The `promoted_from` exposure to team members is read from source, not probed.
- Project paths, unverified: the independent review read in source that `live_project_memories` returns every file memory with a given `project_id` regardless of owner, and that account import validates no `project_id`. If so, an import with a foreign `project_id` and `supersedes` could add a row to another account's project view and hide a same-project target. This was not probed, so it is not claimed here. The caller list of the fix includes the shared-project and project-conflict helpers only because they use the same helper; the project security question is not fixed or closed.
- The audit probe did not use a PostgreSQL database. The only PostgreSQL run is the ordering test under Fixed.
