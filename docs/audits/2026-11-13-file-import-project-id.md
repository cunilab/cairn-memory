---
id: CM-AUDIT-014
title: "File import project_id ownership — 2026-11-13"
status: draft
---

# File import project_id ownership — 2026-11-13

This note checks the project-import gap left unverified by [CM-AUDIT-012](./2026-11-13-file-supersedes-ownership.md) and queued in [CM-AUDIT-013](./2026-11-13-file-import-tombstone-ownership.md): in the file store, whether an account import can add a memory that carries another account's `project_id` and change that account's project view. The audit step changed nothing. The finding was then fixed (see Fixed), with the policy the user chose. No dependency, schema or lock change. PostgreSQL was not reviewed.

## Progress

| Item | Status |
| --- | --- |
| Account import of a row carrying another person's `project_id` (file store) | Reproduced with a core API probe, then fixed 2026-11-13 with the user's policy. Independently accepted (file store only, see Fixed). |
| Foreign `supersedes` through the owner project view | Reproduced in the same probe. Fixed with the same change (stored foreign rows are skipped in the owner views). Independently accepted (file store only). |
| Independent review: B1 (`promote_memory` returned a stored foreign row) and S1 (`share_project_memory` accepted one) | Reproduced by the reviewer, fixed 2026-11-13 (see Fixed). Independently re-checked and accepted. |
| PostgreSQL import and project views | Not reviewed. |
| `project_maintenance` caller check | Fixed in the file store and independently accepted (maintenance only). See Adjacent. |
| Unowned legacy `/forget` ([CM-AUDIT-013](./2026-11-13-file-import-tombstone-ownership.md) finding 2) | Fixed and independently accepted there (file store). |
| Other Layer 4 registry endpoints, old-data repair | Registry access is a separate task (see CM-AUDIT-014 follow-up). Not expanded here. |

## Path

- `import_for_account` (`crates/core/src/lib.rs:3960`) checks only that an imported memory or observation id is not owned by another person. `normalize_export_bundle` (`:4803`) validates ids, sizes, redaction and lifecycle. Nothing checks `project_id`, `scope`, or whether the project exists or belongs to the caller. A row naming a nonexistent project is stored too.
- `live_project_memories` (`:2913`) selects every memory with the project id and drops a row when any other same-project memory has `supersedes` pointing at it. It does not look at the owner of either row. `superseded_targets` (`:5622`, CM-AUDIT-012) has the owner rule, but this helper does not use it.
- Callers of `live_project_memories`: `project_memories` (`:2121`, owner-only list, route `GET /cairn-memory/projects/{id}/memories`), `share_candidates` (`:2501`), and `project_conflicts` (owner branch). `project_maintenance` (`:1556`) builds its own same-project list the same way.
- The team view `shared_project_memories` selects by recorded shares and uses `live_superseded_targets`, so it is not affected by an unshared row.
- The HTTP routes are thin: `/cairn-memory/import` calls `import_for_account` for an account bearer (`crates/server/src/lib.rs:2475-2489`) and `list_project_memories` calls `project_memories` (`:1906`).
- Expected behavior: project memory is created by the owner's explicit promotion from the owner's own memory ([ADR 0009](../adr/0009-explicit-project-promotion.md) decisions 1-2), the owner-only list shows promoted decisions ([project-layer4](../contracts/project-layer4.md), [share-layer5](../contracts/share-layer5.md)), and share candidates are the owner's promoted memories ([share-layer5](../contracts/share-layer5.md) share candidates).

## Findings

### 1. A row owned by one account can enter another account's project view — medium, confidence high, fixed, independently accepted (file store)

Reachability: any registered account that knows the target `project_id`. Ids are `project-` plus 24 hex characters, so an outsider cannot guess one. A current or former member knows it, and so does anyone who was told it. The probe used a member and an outsider with the id supplied.

Reproduction, 2026-11-13, synthetic file store under `/tmp`, three accounts through `MemoryStore` (core API; the routes above are thin): `/tmp/cm-project-import-probe/src/main.rs`, run with `cargo run --offline`.

| Step | Result |
| --- | --- |
| Control: A registers a project and promotes a memory; outsider B lists it | B gets `account does not own the project`. A lists one row. The invited member M sees the shared row. |
| Control: B imports its own row into B's own project; A imports its own row into A's project | Each appears only in its own owner list. A's list is unchanged by B. |
| B imports a B-owned row with `project_id` = A's project | Import 200, 1 imported. A's owner project list now shows the row. A's recall does not (account recall keeps owned rows). |
| Same state | A's share candidates for M list B's row. `project_maintenance` called by B lists it as active. M's shared view is unchanged. |
| A shares that candidate to M (a deliberate owner action) | M's shared view then contains B's text. |
| B imports a row with A's `project_id` and `supersedes` = A's promoted memory | A's owner project list and share candidates drop A's memory. `project_maintenance` reports it as superseded. A's recall and M's shared view still show it (the CM-AUDIT-012 rule holds there). |
| Invited member M does the same to another A project memory | Same result: A's list no longer shows it. |
| B imports a row with a nonexistent `project_id` | Import accepted. |
| Reopen the store | Same lists. |

Impact:

- Integrity of the owner's project view: foreign text appears as a project decision, and the owner's own project memory can be hidden from the owner list, the share-candidate list and conflict checks. Nothing is deleted, and the owner's recall and the shared team view are not changed.
- Foreign text can reach the team only if the owner accepts the candidate. The candidate list does not say the row is not the owner's.
- Not shown: a foreign account reading the owner's project memory through this path.

### Adjacent, not part of this finding

`project_maintenance` (`crates/core/src/lib.rs:1556`) only authenticates the caller. It was later audited on its own and reproduced in the file store: see `/tmp/cm-maintenance-access-audit.md` (an outsider, an invited member and a removed member who know the project id all receive the active, superseded and stale memory ids of the whole project, including unshared memories; no content). The user then chose owner-only access for maintenance and, separately, the registry endpoints; members use explicit sharing. Maintenance is fixed 2026-11-13 (file store): after authentication `project_maintenance` requires `require_project_owner` plus an owner record on the project, so a current member, a removed member, an outsider and an ownerless project get `account does not own the project`, with no memory id or commit in the error; the owner's report and the `project_rows` selection are unchanged and the call writes nothing. Core `project_maintenance_is_owner_only_and_needs_an_owned_project` and server `project_maintenance_is_owner_only_over_http` failed first (the member and the HTTP outsider got the report; with the ownerless check removed on a copy the ownerless owner got it) and pass. After: `cargo test -p cairn-core --lib` (55), `cargo test -p cairn-server --lib` (34), fmt, clippy (core, storage, server), the document validator; PostgreSQL, smoke scripts and a live server were not run. [project-layer4](../contracts/project-layer4.md) and ADR 0008 (draft) state the rule. Independently accepted (file store, maintenance only; reviewer role probe plus focused tests, implementer core 55 / server 34 / fmt / clippy / validator). One caveat the reviewer recorded: an ownerless project is claimed by the first account that registers the repository, which is existing behavior and not proof of the original owner. The registry endpoints (branches, worktrees, entities, freshness, branch and worktree registration, branch close) stay open and queued, and the PostgreSQL maintenance path is source-read and unchanged.

## Options considered

The smallest fix candidates were:

1. In `import_for_account`, reject a row whose `project_id` is not a project the caller owns (the same error as a foreign row), and give `live_project_memories` the owner rule that `superseded_targets` already has. Smallest and consistent with CM-AUDIT-012 and 013.
2. Make `live_project_memories` list only rows owned by the project owner, without an import check. This keeps old foreign rows out of the list but still stores them.
3. Leave import alone and document that project views are advisory. Not recommended: the list is owner-facing and feeds sharing.

The user chose: owner-only project-linked imports, reject the whole import for a foreign or a missing project, and exclude already stored foreign rows from the owner views without deleting them.

## Fixed

2026-11-13, file store only. `import_for_account` (`crates/core/src/lib.rs`) now rejects the whole bundle, after authentication and the existing row-ownership check and before any tombstone filter, session binding or merge, when any memory row has a `project_id` that is not owned by the caller (`project_owner_id` equals the caller). A foreign, missing and unowned project all return one error, `account does not own the project`, with no id, owner or payload. Only memory rows carry a project id; observations and sessions do not, and the bundle holds no project record. Import does not create a project, map one, or strip the link, so a cross-store restore needs the matching project registered by the same account first. An invited member is not the owner and is rejected. A project with no owner record is rejected until its owner claims it (registering the same repository claims it). Unscoped `import_bundle`, the local CLI, MCP and a store with no accounts are unchanged, and so is PostgreSQL.

Stored foreign rows: `project_rows` selects the rows with the project id that the project owner owns or that have no owner, and `live_project_memories` and `project_maintenance` use it with the existing `superseded_targets` rule. The owner list, share candidates, conflict check and maintenance report therefore skip a row another account owns, and such a row cannot hide the owner's memory. Rows are not deleted or rewritten, the owner's recall and the team shared view are unchanged. A row with no owner stays visible (trusted legacy). Side effect of the shared supersession rule: a successor that has an owner no longer hides an unowned target in these project views, the same as CM-AUDIT-012 already does for recall.

Tests, written first and failing before the change: core `account_import_rejects_project_rows_the_caller_does_not_own` (outsider, invited member and missing project reject a bundle that also holds a valid row and a tombstone; file bytes identical, no owners or tombstones added, reopened store empty; a bad token fails on authentication first; an ownerless project is rejected until claimed, then the own import works) and `stored_foreign_project_rows_stay_out_of_owner_project_views` (a foreign row, a foreign row that supersedes the owner's memory, a foreign contradicting row and an unowned row: only the owner's and the unowned row are listed, candidates, conflicts and maintenance agree, recall and the member's shared view are unchanged, the outsider's three rows are still stored, same after reopening). Server `project_linked_import_is_owner_only_over_http`: 401 with the ownership error, no payload, file bytes identical; the owner's own import returns 200. The HTTP test failed with the check disabled on a copy (200 instead of 401). Also passed: `cargo test -p cairn-core --lib` (50), `cargo test -p cairn-server --lib` (32), `cargo fmt --all -- --check`, `cargo clippy -p cairn-core -p cairn-storage -p cairn-server --all-targets -- -D warnings`. Not run: PostgreSQL tests, smoke scripts, a live server.

Docs updated, all still `draft`: [export-v1](../contracts/export-v1.md), [auth-layer3](../contracts/auth-layer3.md), [ADR 0008](../adr/0008-self-hosted-account-authentication.md), [project-layer4](../contracts/project-layer4.md), [share-layer5](../contracts/share-layer5.md). Independent security review of the first version of this change, 2026-11-13: NOT ACCEPTED, one blocker (B1) and one same-class gap (S1), both reproduced on a synthetic state, then fixed below. The follow-up was independently re-checked and accepted (see the end of this section). Still open: the `project_maintenance` caller check (any authenticated account can call it), PostgreSQL, old-data repair, and CM-AUDIT-013 finding 2.

B1 and S1, 2026-11-13. B1: `promote_memory` looked up an existing promotion by `project_id` and `promoted_from` over every stored row, so a stored foreign row naming the owner's unpromoted source was returned as the owner's promotion, no real promotion was created, and a later confirm failed with `unknown memory or event: <foreign id>`. S1: `share_project_memory` found a row by id with no owner check, so the owner could share a stored foreign row to a member. Fixed minimally: the promotion lookup now searches `project_rows` (rows the project owner owns, plus rows with no owner), and `share_project_memory` calls `reject_other_owner` before `shareable_project_memory`, the guard `promote_memory` and `verify_project_memory` already use (`verify_project_memory` was guarded already). A foreign id now returns `unknown memory or event: <id>`. An unowned local promotion row is still returned as the owner's idempotent promotion. New core test `stored_foreign_project_rows_do_not_block_promotion_or_reach_a_share` failed first (confirm hit the foreign row; with only the share guard removed on a copy it failed at the foreign share) and passes: a real owner promotion is created and repeats idempotently, listed, confirmed and owned; the foreign rows stay stored and exported by their owner; a foreign direct share and verify are rejected with no new share; the owner's own share reaches the member; an unowned local row stays the idempotent promotion. Checks after: `cargo test -p cairn-core --lib` (51), `cargo test -p cairn-server --lib` (32), fmt, clippy (core, storage, server). Independent re-check, 2026-11-13: ACCEPT for finding 1, file store only. The reviewer read the corrected `promote_memory` and `share_project_memory`, re-ran the B1/S1 probe on the current tree (a real owner promotion is created and `confirm_memory` succeeds; a direct share of a foreign id fails with `unknown memory or event`; the foreign rows stay stored and exported by their owner; the import policy and the unowned-row rules are unchanged) and ran four narrow tests (`stored_foreign_project`, `account_import_rejects_project_rows`, promote/share, `unscoped_import`), all passing; the reviewer did not rerun the full suites, fmt, clippy or the document validator. The implementer's checks were core 51, server 32, fmt, clippy (core, storage, server) and the document validator. Foreign rows were injected by editing synthetic state, since the public API can no longer create them. This accepts finding 1 only, not the whole audit; the `project_maintenance` caller check, PostgreSQL, old-data repair and CM-AUDIT-013 finding 2 stay open.

## Limits and uncertainties

- Core API probe only; no live HTTP run, no PostgreSQL. PostgreSQL import and project views are unreviewed.
- A single run per scenario.
- Whether an outsider can learn a project id by another path was not tested.
- Existing stores may already hold such rows. They are hidden from the owner views, not found, removed or repaired.
- Other readers of a row's `project_id`: `promote_memory` and `share_project_memory` were found to use a stored foreign row and are fixed (B1, S1). `verify_project_memory` already rejected a foreign id. `dismiss_share_candidate` finds a row by id without an owner check but only records a dismissal for the supplied id; not treated as a bypass and not changed.
- Rows owned by a person other than the project owner are hidden from the owner views even if the owner meant to keep them; no legitimate flow creating such rows was found.

## Evidence

- Source reads at the lines above, 2026-11-13, working tree with uncommitted changes.
- Probe crate `/tmp/cm-project-import-probe` (the pre-fix reproduction) (path dependency on `crates/core`, `CARGO_TARGET_DIR=/tmp/cm-audit013-review/target`, `cargo run --offline`). It used synthetic stores under `/tmp`, removed afterwards. Ports 3000 and 62200, real stores and PostgreSQL were not used.
