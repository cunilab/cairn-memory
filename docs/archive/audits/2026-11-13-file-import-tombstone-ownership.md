---
id: CM-AUDIT-013
title: "File import tombstone ownership — 2026-11-13"
status: draft
---

# File import tombstone ownership — 2026-11-13

This note checks one residual left open by [CM-AUDIT-012](./2026-11-13-file-supersedes-ownership.md): in the file store, whether an account import can add forget tombstones (`forgottenIds`) for rows it does not own. Read-only audit: no application, dependency, schema, or lock change. It does not review the rest of import, lifecycle, or sharing.

## Progress

| Item | Status |
| --- | --- |
| PostgreSQL search cap and foreign `supersedes` (CM-AUDIT-011, CM-AUDIT-012) | Closed in those notes. Not rechecked. |
| Foreign `forgottenIds` on file account import | This note. Reproduced over HTTP, fixed 2026-11-13 with option 1 as the user approved. Finding 1 is fixed and independently accepted: the first review accepted the filter and found one blocker (B1) and one wording issue (N1); the re-check (`/tmp/cm-audit013-review/b1-recheck.md`, 2026-11-13) accepted the correction. Finding 2 (unowned legacy `/forget`) is still OPEN, and this does not accept the rest of import. |
| Finding 2 (account forgets an unowned legacy row) and finding 3 (tombstone of another account's ids through `source_event_ids` of the caller's own row) | Both reproduced 2026-11-13, then fixed 2026-11-13 with the policy the user approved (strict ownership for account forget and every source cascade). Independently accepted for the file store (not all import or lifecycle security); see Fixed, findings 2 and 3. Finding 1 was accepted separately. |
| Project views and import `project_id` | Reproduced and fixed in the file store in [CM-AUDIT-014](./2026-11-13-file-import-project-id.md); finding 1 there is independently accepted (file store). Not part of this note. |
| Old-data republishing, broad lifecycle and retention, sharing, code-quality, docs, and unused-code audits | Still queued. Not reviewed. |

## Path (pre-fix reading)

This section and the Trigger, Impact and reproduction of finding 1 describe the code before the fix, with its line numbers. The fix is under Fixed.

- `StoreState.forgotten_ids` is one `HashSet<String>` for the whole file (`crates/core/src/lib.rs:434`). It holds no person. File ids are globally unique: a memory id or event id belongs to at most one row, and the first owner wins (`record_memory_owner`, `:2672`).
- `import_for_account` (`:3938-3997`) checks that every imported observation and memory id is not owned by another person, then merges, then records the caller as owner of those ids. It never looks at `bundle.forgotten_ids`.
- `import_bundle` (`:3999-4075`) copies the bundle's `forgotten_ids` into the set (`:4052-4058`) with no check at all. It is all-or-nothing: it builds a candidate state and replaces the real one only if every row merged.
- A tombstone hides by id, and also by source id: `memory_forgotten` (`:5554-5560`) is true when the memory id or any of its `source_event_ids` is in the set. Search, resume, unresolved, shared project views, and contradiction targets use it. Device sync pull also drops forgotten event ids (`:3401`). The session summary skips a forgotten observation (`:3642`).
- Unknown ids are accepted. The route `forget_for_account` is stricter: `hide_for_account` (`:4394`) rejects an id owned by someone else and `hide_id` (`:4477`) rejects an id that does not exist. Only import skips both checks.
- `export_owned` (`:3871`) lists a tombstone only when the exporter owns the memory or observation, or the id is a source id of an exported memory. `sweep_expired` (`:981`) adds expired ids to the same set, so an export contains tombstones the person never asked for. A tombstone for an explicit memory's own source event id has no observation row: `remember` stores a source id with no observation.
- Unscoped paths (local CLI and MCP, and the HTTP route when the store has no accounts) call `import_bundle` directly. They are trusted by design.
- PostgreSQL scopes tombstones by person: `cairn_forgotten_items` has primary key `(person_id, item_id)` (`migrations/0001_layer3_personal_memory.sql:111-116`), the import binds the authenticated person (`crates/storage/src/lib.rs:3100-3109`), and every search and recall predicate joins `f.person_id = m.person_id`. A foreign tombstone is inert there, now and later. Source only; not run.

## Findings

### 1. Account import accepts tombstones for ids the account does not own — high, confidence high, fixed, independently accepted

Path: `crates/core/src/lib.rs:4052-4058`, reached from `:3938-3997` and the route `/cairn-memory/import` (`crates/server/src/lib.rs:2475`).

Trigger: any registered account sends an import bundle whose `forgottenIds` names another account's memory id or observation event id, or an id that does not exist yet.

Impact:

- The owner's search, context and resume stop returning the row, and the owner has no way to undo it: there is no un-forget. The row stays in the owner's export, so data is not destroyed, but it is hidden for good.
- A tombstone on an observation event id hides every memory built from it, for example the owner's session summary.
- A tombstone for an id that does not exist yet waits. When the owner later captures that event id, the request returns success and the event is silently dead: it is skipped by the next summary and by sync pull.
- Nothing from the owner is disclosed to the importer. The owner's own export later lists the foreign tombstone as if it were theirs, so a restore elsewhere makes it permanent.
- Event ids are chosen by the client, and shared project memories carry the source memory id to team members, so an id is not a secret.

Reproduction, 2026-11-13: the real `cairn-server` built from the working tree, on temporary ports with new store files, two accounts over HTTP. Probes `/tmp/cm-audit013-forgotten-probe.py` and `/tmp/cm-audit013-forgotten-probe2.py`. Nothing sensitive was printed; ids are shortened.

| Step | Result |
| --- | --- |
| A owns two memories; B imports `forgottenIds` with A's first memory id | HTTP 200. A's search returns one row instead of two. A's export still holds the row and now lists the tombstone. B's own export does not list it. |
| A ends a session whose observation event id is `ev-a-1`; B imports `forgottenIds: ["ev-a-1"]` | HTTP 200. A's resume goes from one summary to none. |
| B imports `forgottenIds: ["ev-a-later"]` (no such id); A later observes that id | Import 200, `imported_forgotten_ids: 1`. A's observe returns 200, and A's session summary does not include that observation. |
| B imports A's observation as a row | HTTP 401 `account does not own the memory` (the existing row-id check). |
| B imports a row with an unknown id | HTTP 200. |
| A forgets one memory and exports | The export has 2 tombstones and 2 memory rows. One tombstone is the forgotten memory's source event id and no row in the bundle has that id. |
| No accounts, unscoped import with an unknown tombstone | HTTP 200, 1 imported (trusted path, unchanged). |

### 2. An account can forget a row that has no owner — low to medium, confidence high, fixed, independently accepted (file store) (reproduced 2026-11-13)

Path: `forget_for_account` and `delete_for_account` (`crates/core/src/lib.rs:4468-4474`) call `hide_for_account` (`:4476`). It calls `reject_member_mutation` (`:3073`), which is `reject_other_owner` (`:2739`), and that passes when `item_owner` has no owner. `hide_id` (`:4559`) then only checks that the id exists, inserts it and the memory's `source_event_ids` into the global tombstone set, and `note_sync_deletions` and an audit event (`memory_forget`) are written for the caller. The routes `/cairn-memory/forget` and `/delete` (`crates/server/src/lib.rs:2362-2402`) take this path whenever the store has accounts (`file_bearer`, `:2622`). Without accounts, and in the local CLI and MCP (`mcp.rs:318-330`), `forget` is the unscoped `MemoryStore::forget` (`:4461`), which has no owner check by design.

Unowned rows exist in a file that has accounts: data from before `bootstrap_account`, or written by the local CLI or MCP. Bootstrap does not claim them. No account can see them: account recall, resume, summaries and export keep only owned rows. The only claim path found is an identical re-import (a recognised prior limitation).

Reproduction, 2026-11-13, core API on a synthetic store under `/tmp` (probe `/tmp/cm-unowned-forget-probe`, output `output.txt`, `cargo run --offline`). Legacy rows were written with the unscoped calls before two accounts were created.

| Step | Result |
| --- | --- |
| B forgets a legacy explicit memory, a legacy session summary (`memory-<session>`) and a legacy event id | All three return success. |
| B forgets an unknown id, A's memory id, A's event id | All three: `unknown memory or event: <id>` (the same text, so no oracle between unknown and foreign). The unowned case differs: it succeeds, so success reveals that a legacy id exists. |
| Effect | The unscoped search no longer finds the legacy memory, and the unscoped export lists four tombstones (the ids and the explicit memory's source id). It persists after reopening. A's recall, resume and export are unchanged. B's export shows no tombstone and no row. B's own audit list records the three ids. |

Impact: no other account is affected. The local operator's legacy rows are hidden from the unscoped CLI and MCP and cannot be un-forgotten; the stored rows and the unscoped export remain. The caller must know a legacy id: summary ids are `memory-<session id>` and event ids are client-chosen or hashed, so they are guessable only in part. Severity depends on whether such files exist, which is a deployment question. Intended trust boundary: the docs say only the owner forgets (`auth-layer3.md`), capture and sync reject an unowned existing id, and account views never show unowned rows; forget is the one account path that still treats an unowned id as allowed. No contract names the forget rule for an unowned id.

### 3. A caller can tombstone another account's ids through the `source_event_ids` of its own row — medium, confidence high, fixed, independently accepted (file store) (reproduced 2026-11-13, found while auditing finding 2)

Path: `import_for_account` (`:3953`) checks only a row's own id ( finding 1 above); a row's `source_event_ids` are stored as given. `hide_id` (`:4559`) then inserts every `source_event_ids` string of the forgotten memory into the global tombstone set with no owner check. `memory_forgotten` (`:5636`) hides any memory that lists a tombstoned id as a source.

Reproduction, same probe: B imports a B-owned memory row whose `source_event_ids` are A's event id and A's explicit memory id (accepted, 1 imported), then B forgets its own row (success). Afterwards A's recall of its summary and of its explicit memory is empty and A's resume is empty. A's export still holds both rows (nothing is deleted). A's own forget still works. Prerequisite: B must know an id; ids are not secret (CM-AUDIT-013 finding 1). The same route can tombstone a future event id of A, which poisons A's later capture as in B1. That future-id variant was source reading only when this finding was written; it is reproduced by the fix test below (before the fix, a future id named as a source was tombstoned).

This bypasses the finding 1 fix by another route and is why finding 1 is accepted for the import tombstone filter only.

Options considered (the user chose a stricter variant of option 3, see Fixed, findings 2 and 3):

1. Reject, in `import_for_account`, a row whose `source_event_ids` contains an id another person owns, with the existing ownership error. Closes existing ids, not future ids.
2. In `hide_id`, cascade a source id only when it is not owned by another person. Closes existing ids, not future or unowned ids.
3. In `hide_id`, cascade a source id only when the caller owns it, and record an owner for the source id that `remember_for_account` generates (`event-explicit-<time>`, no observation row) so that a normal forget still cascades. Closes existing and future ids. Needs a decision on backfilling old explicit memories, whose generated source id has no owner: their cascade would be skipped until backfilled.

The user approved: strict ownership for account forget and delete and for every source cascade, owner records for newly generated explicit source ids, and no backfill.

### Fixed, findings 2 and 3 (2026-11-13, file store only)

`hide_for_account` (`crates/core/src/lib.rs`) keeps its authentication and `reject_member_mutation` first, then requires `item_owner(id)` to be the caller; an unowned, foreign or unknown id fails with `unknown memory or event: <id>` and writes nothing. `hide_id` takes the owner and cascades only the memory's source ids the caller owns; it returns the ids it tombstoned and `note_sync_deletions` records deletions for those only, so a foreign, unknown or future source id leaves no tombstone, deletion or audit trace. The trusted unscoped `forget` passes no owner and cascades every source id as before. `remember_for_account` now stages the row, records the memory owner and the owner of the source id it generated (`record_generated_source_owners`, no observation row), and persists once, rolling the in-memory row and both owner records back on a failed rename; before, it wrote twice. `correct_for_account` records its generated correction source in its existing write. Because an owner record needs no row, `id_owned_by_other` (renamed from `memory_id_owned_by_other`) now checks memory and observation owner records, so `capture_for_account` and `sync_push` refuse an id another person owns even when no row exists; without it another account could have created an event row under an owned generated source id. The import row check already uses `item_owner` and rejects such an id. No backfill, no rebind, no format or schema change, and the approved import tombstone filter is unchanged. Old data is not repaired: a memory owned before this change keeps an unowned generated source id, which stays unclaimed and is not tombstoned when the memory is forgotten. Consequence: a copy that shares that source (for example a promoted copy) is no longer hidden through the source of a forgotten old memory. A restore into another store claims only imported row ids.

Tests, written first and failing before the change: `account_forget_and_delete_require_an_owned_id` (legacy event, summary and explicit memory, a foreign and an unknown id: same error for forget and delete, file bytes, tombstones, sync deletions and audit unchanged, a bad token fails, the unscoped call and the owner's own delete still work), `forget_cascades_only_sources_the_caller_owns` (a row naming a foreign event, a foreign memory and a future event: forgetting it tombstones only the row, the owner's recall, resume, device pull and later capture of the future id are unaffected, also after reopening; an owned observation and the generated source still cascade; an old owned memory with an unowned source is forgotten without tombstoning or claiming the source) and `generated_explicit_source_is_owned_without_a_row_and_rolls_back` (owner record persisted, no row, another account's capture, push and import of that id rejected, a blocked rename leaves no row or owner record); server `account_forget_requires_an_owned_id_over_http` (failed on a copy with the owner check removed). After: `cargo test -p cairn-core --lib` (54), `cargo test -p cairn-server --lib` (33), `cargo fmt --all -- --check`, `cargo clippy -p cairn-core -p cairn-storage -p cairn-server --all-targets -- -D warnings`, the document validator. Not run: PostgreSQL, smoke scripts, a live server. Independent review, 2026-11-13: ACCEPT findings 2 and 3 for the file store, with two documentation follow-ups (D1, D4), applied here and in [auth-layer3](../../contracts/auth-layer3.md). The reviewer read the change and ran an extended probe (P1 to P8: legacy forget and delete, foreign, unknown and rowless ids, own rows naming foreign, future and generated sources, the generated-source reservation against another account's capture, push and import, a correction source, an old owned memory simulated by removing its source's owner record) plus ten core tests and the server test; it did not rerun the full suites, fmt, clippy or the document validator, and edited synthetic state to simulate old data. The implementer's checks (core 54, server 33, fmt, clippy, document validator) are separate. This accepts findings 2 and 3 only, not all import or lifecycle security. Old-data poison repair, id squatting, the two-write sequence in `correct_for_account`, PostgreSQL and the `project_maintenance` caller check stay open.

Old-data consequence (D1, reproduced by the reviewer on a simulated old state, no id disclosed and nothing deleted): for an owned memory made before this change, the generated source id has no owner record and no row, so another account that already knows that exact id can still capture it, and forgetting its own new event hides the old memory from its owner's recall. It was possible before this change, so it is not a regression; it follows from the approved no-backfill decision and is not repaired. The id is `event-explicit-` plus a nanosecond time and is not shown to other accounts. The approved no-backfill choice also means that, for such a memory, a forget tombstones the memory id only.

Fixed 2026-11-13 with option 1, as approved by the user. `import_for_account` (`crates/core/src/lib.rs`) now filters `bundle.forgotten_ids` after the existing row-ownership checks and before `import_bundle`:

- an id owned by the caller is applied (`item_owner` equals the caller);
- an id that is a memory or observation id in the same bundle and has no owner is applied (the row becomes the caller's);
- an id owned by another person rejects the whole import with `account does not own the memory`, the same text as a foreign row, with no id or payload in it (a foreign row with the same id as a tombstone is rejected earlier by the row check, so foreign wins);
- every other id is ignored and not counted, including a source event id that has no row. A row's `source_event_ids` are not proof, since a row can name a foreign or future id.

`import_bundle` already builds a candidate and writes nothing when it fails, and the filter runs before it, so a rejected import writes nothing. Unscoped `import_bundle`, the local CLI and MCP, and a store with no accounts are unchanged. The PostgreSQL import and its person-keyed tombstones are unchanged. No type, field, format or schema was added. The foreign `supersedes` rule is untouched. Tombstones already stored are not identified, removed or migrated, so a row hidden earlier stays hidden. The user approved ignoring an unknown id: a tombstone-only bundle for an id with no owner and no row is not kept, so this does not claim a full tombstone-only partial backup.

Checks, 2026-11-13. The tests were written first and run before the fix: three core tests failed (the unknown-id count was 3 instead of 1, the restore count was 2 instead of 1, and the foreign import was accepted), then all passed. The HTTP test failed on a copy of the old behaviour (status 200 instead of 401) and passed with the fix.

- `foreign_tombstones_reject_the_whole_import_and_change_nothing`: a foreign memory id and a foreign event id, each with a valid new row, reject with the ownership error; the file bytes are identical afterwards; a reopened store gives the owner the same search and resume, the other account has no new row, and the tombstone set is empty.
- `unknown_tombstones_are_ignored_and_source_ids_are_not_permission`: an unknown id, a source id named by the importer's own row, and that row's id are imported; only the row's own id counts and is stored; the owner's later capture of the named source id and its summary are not affected, also after reopening.
- `own_tombstones_apply_and_forgotten_memories_survive_a_restore`: a tombstone-only import for the caller's own row and a same-bundle new row with its tombstone apply (2 counted); a normal export of a forgotten memory restores into a new store and account with the memory still forgotten, the source id with no row ignored (1 counted), and the kept memory still found. The restore fixture has no session rows: account import rejects another person's session `person_id` by the existing policy.
- `unscoped_import_still_accepts_any_tombstone`: no accounts, unknown tombstone, 1 counted.
- `foreign_tombstone_import_is_rejected_and_unknown_ones_are_ignored_over_http` (server crate): `/import` returns 401 without the owner's content, the store file is byte-identical, the owner's search is unchanged, and an unknown id returns 200 with 0 counted.

B1 and N1, 2026-11-13. The independent review (`/tmp/cm-audit013-review/report.md`) accepted the filter and found that memory ids and event ids share one id space while `capture_for_account` and `sync_push` looked only at observation owners. An importer could bring a memory row named like the owner's future event id, with a tombstone (allowed as a brought row), and the owner's later capture returned success but the event was dropped from summaries. Corrected: a small helper `memory_id_owned_by_other` makes both calls reject an event id that a memory row of a different person owns, with the existing `event belongs to another person: <id>` error, after authentication and before any capture, persist or sync event. The device push persists or restores its last-seen time as the other foreign-event rejections do. A memory row owned by the same person, or by nobody, behaves as before; the import filter is unchanged. Test `a_foreign_memory_row_id_cannot_poison_a_later_capture_or_sync_push` was written first and failed (capture accepted the id), then passed: account capture and device push both fail with that error and no payload, no observation, sync event or cursor is added, memory and reopened disk agree on last-seen, a bad token fails on authentication, a normal capture, device adoption and the same person's own memory row under an id still work, and an observation owned by another person still cannot be claimed as a memory row. The HTTP test gained the same case (`/observe` is rejected, no private content in the body) and failed on a copy with the guard disabled. N1: the wording that the error does not say whether an id exists was wrong and is corrected here, in [ADR 0008](../../adr/0008-self-hosted-account-authentication.md), [auth-layer3](../../contracts/auth-layer3.md), [export-v1](../contracts/export-v1.md), `MEMORY.md` and the code comment: the foreign-id error shows that an id is owned, with no payload or owner. The first owner still wins, so an importer can reserve an unused id with its own row; the scope here closes the silent poisoning, not the availability of arbitrary ids. Also passed after B1: `cargo test -p cairn-core --lib` (48), `cargo test -p cairn-server --lib` (31), fmt, clippy (core, storage, server). Earlier: `cargo test -p cairn-core --lib` (47), `cargo test -p cairn-server --lib` (31), `cargo fmt --all -- --check`, `cargo clippy -p cairn-core -p cairn-storage -p cairn-server --all-targets -- -D warnings`. Not run: PostgreSQL tests, smoke scripts, a live server, other crates. Device sync pull end to end was not run; the id is kept out of the tombstone set, which is what sync pull reads. Docs updated: [export-v1](../contracts/export-v1.md), [auth-layer3](../../contracts/auth-layer3.md) and [ADR 0008](../../adr/0008-self-hosted-account-authentication.md), all still `draft`. Independent re-check, 2026-11-13: ACCEPT for finding 1 only. The reviewer re-ran the exploit on the current tree (a memory row named like another account's future event id, with a matching tombstone): account capture and device sync push both fail with `event belongs to another person`, no observation, sync event or cursor is created, the file changes only by the persisted device last-seen time, and the result is the same after reopening; a bad token fails on authentication first; normal capture, duplicate, device adoption and the same person's own row still work. Focused checks it ran: core 6 and server 1 (`a_foreign_memory_row`, `tombstone`, `foreign_tombstone_import`); it did not rerun fmt, clippy or the full suites. Limits it recorded: already-colliding old data now errors on retry instead of returning a duplicate (not repaired); a deterministic summary-memory-id reservation needs the victim's person id (unverified). This accepts finding 1 only, not the whole audit or import security. Findings 2 and 3 were later reproduced and fixed (see above). Still open: old-data repair.

## Options

All three keep PostgreSQL, the export format and unscoped trust as they are.

1. **Apply a tombstone only for ids the caller owns or brings (recommended).** In `import_for_account`, before the merge:
   - an id owned by the caller, or equal to a memory or observation id in the same bundle, is applied;
   - an id owned by another person is rejected, with the same text the row check uses. The error has no payload or owner, but it does show that the id is owned by someone, unlike an unknown id (accepted, 0 counted);
   - any other id (no owner, no row in the bundle) is ignored, not stored and not counted. This includes source event ids with no row, which a normal export contains (last probe row), so a legitimate round trip is not rejected.
   - All checks run before `import_bundle`, which already writes nothing if the merge fails. Unscoped import is not changed. No format or schema change.
2. **Account-scoped tombstones.** Store the person with each tombstone, like PostgreSQL. This needs a changed `forgotten_ids` type or a new side list, and the readers change: `memory_forgotten`, sync pull, `export_owned`, `sweep_expired`, `hide_id` and the project paths. It adds a stored-format question. Not needed: ids are globally unique and the first owner wins, so once only an owner can add a tombstone for an id, a per-person key adds nothing.
3. **Keep foreign tombstones but make them inert.** This needs the same per-tombstone person as option 2, and it cannot tell old foreign tombstones from honest ones. It also keeps unknown-id poison stored.

Recommendation: option 1. It is the smallest change, it matches the personal boundary in the product docs and the existing rule that import cannot take another person's id, and it keeps the export contract: a tombstone in a normal export always refers to the exporter's own row or to that row's source id. It closes the silent poisoning because a tombstone can never name an id the caller does not own or bring, and ownership never moves. It does not stop an importer from bringing a row under an unused id and so reserving it (see finding 1 B1). A plain owner check would not be enough: it would reject the legitimate source-id tombstones, and a rule that accepted any source id would let an importer name a foreign or future id as a source of its own row.

Choices for the decision maker:

- **Foreign-owned id: reject the whole import, or ignore it.** Reject is consistent with the row check and tells the caller something is wrong. It confirms that an id exists, which the row check already does. Recommend reject.
- **Unknown id: ignore, or reject.** Rejecting breaks restoring a normal export. A tombstone-only bundle for an unseen id (negative knowledge for a row not yet restored) is dropped by ignore. That is a small loss; no export produces it. Recommend ignore, and say so in the contract.

## Remedy scope as planned (historical, pre-fix)

This was the plan. The current code and tests are under Fixed.


- One helper in `crates/core/src/lib.rs` called from `import_for_account` before `import_bundle`: collect the ids the bundle brings (observations, memories) and the caller's owned ids, then filter `bundle.forgotten_ids`, return the same error as `reject_other_owner` for a foreign-owned id, and drop the rest. About twenty lines. No new field, no new type.
- Callers: only `import_for_account` (the `/cairn-memory/import` route). `import_bundle` stays unscoped and trusted. The PostgreSQL import is unchanged.
- Tests to add, with the rule disabled first: a foreign memory id, a foreign observation id and an unknown id do not hide anything and do not reach the owner's recall, resume, sync pull or later capture; the response and the error contain no payload; a normal export of an account with a forgotten memory imports into a new account and the memory stays forgotten; a tombstone for an owned row not in the bundle still applies; a tombstone-only import of a foreign id is rejected whole and nothing else in the bundle is written.
- Docs: a line in [export-v1](../contracts/export-v1.md) (account import applies a tombstone only for the caller's own ids; other ids are rejected or ignored as stated), a line in [auth-layer3](../../contracts/auth-layer3.md) next to the ownership rule, and a note in [ADR 0008](../../adr/0008-self-hosted-account-authentication.md) if the decision makers treat it as a trust-boundary change.

## Limits and uncertainties

- Existing stores may already hold foreign tombstones. The set has no person and import writes no audit record, so they cannot be found or removed automatically. A person-keyed record (option 2) would not fix that either. Old-data repair stays queued.
- Not probed: the project view effect of a foreign tombstone, device sync pull end to end, and PostgreSQL (source only).
- Ignoring unknown ids is a product choice that drops negative knowledge for rows not yet restored. No code path produces such a bundle today.
- The probe ran once per scenario. Ports 3000 and 62200 and user stores were not used. All servers were stopped.

## Evidence

- Source reads at the lines above, 2026-11-13, working tree with uncommitted changes.
- `cargo build -p cairn-server` finished; the two probe scripts ran against `target/debug/cairn-server` on ports 62743 to 62746 with store files in the session scratch directory. No Rust test, no PostgreSQL run, no source change.
