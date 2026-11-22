---
id: CM-PLAN-019-17
title: "17 Expiry marks, never deletes or hides"
status: frozen
---

# 17 Expiry marks, never deletes or hides

## Goal

A memory is never deleted and never hidden because it expired (user decision 2026-11-23, binding). Expiry only
marks it: the row stays in `memories` with its text, embedding, entities and links; it stays visible in the web and
in `memory_get`, and stays findable by `memory_search`, shown as `expired` with its date. The live 0.1.8 server
deletes today, so this ships right after item 03.

Raw logs are not memories and keep their retention: `observations` (TTL and the 7-day rule), done `jobs`,
`retrieval_log`, and item 13's 14-day search log.

## Out of scope

`memory_forget` (an owner action; see "Delete paths" — it blanks the row, it does not delete it; unchanged).
Changing TTL values (`NOTE_TTL_SECONDS`, `HANDOFF_TTL_SECONDS`, `SYSTEM_TTL_SECONDS`). Restoring rows already
deleted by 0.1.8 (gone; a backup restore is an operator task). Ranking of expired memories (Open question 1).
Internal processing cohorts that skip expired rows (link building `link.rs:27`, consolidation `consolidate.rs:101,218,240`,
promotion live-source count `consolidate.rs:329`, repo/branch moves `repo.rs:109,188`): they do not hide anything from
a reader and stay as they are.

## Decisions it relies on

D-DATA, the 2026-11-23 expiry decision above, D-TOK.

## Delete paths found (main branch `0.1.9` at `02e7d90`)

| # | file:line | What it does | This item |
|---|---|---|---|
| P1 | `crates/store/src/capture.rs:513-518` | Nightly sweep: `DELETE FROM memories WHERE expires_at < now() - EXPIRED_GRACE_SECONDS` (30 days). FK `ON DELETE CASCADE` (`migrations/0001_foundation.sql:115,125,133-134,145,153`) then removes the memory's usage, entities, links, votes rows. **Live data loss.** | Removed |
| P2 | `crates/store/src/capture.rs:427` | Handoff write: `DELETE FROM memories WHERE memory_id = $1 AND owner_id = $2` then re-insert with the same deterministic id, when the old handoff is not kept (item 05 rule). The old handoff's text is lost. | Open question 2 (BLOCKING) |
| P3 | `crates/store/src/memory.rs:562-609` (`forget`) | Owner action. Row stays (`status = 'forgotten'`), text/fields/embedding blanked, `identity_hash` kept; deletes its `entities`, `links` and pending embed/consolidate/link jobs. Not a hard delete of the row; content is removed on purpose by the owner. | Unchanged |
| P4 | `crates/store/src/memory.rs:508` | Vote change: deletes the person's opposite `usage_votes` row before writing the new vote. Replaces a vote, no memory content. | Unchanged |
| P5 | `crates/store/src/capture.rs:531,537,545,551` | Observations, done jobs, retrieval log retention. Raw logs. | Unchanged |
| P6 | `crates/store/src/jobs.rs:136,215,303`, `crates/store/src/instructions.rs:244` | Quarantined job / server_state cleanup. No memory content. | Unchanged |

Paths that only *set* `expires_at` (they become marks once P1 is gone and reads stop filtering):
`capture.rs:524-533` (unused system memory stale 90 days), `consolidate.rs:334` (promoted copy with < 2 live sources),
`repo.rs:285` (handoffs of merged branches), TTLs from `crates/core/src/memory/kind.rs:14`.

Paths that **hide** expired memories from readers (must change): `crates/store/src/memory.rs:23` (`visible`),
`memory.rs:37` (`searchable`), `crates/core/src/project/access.rs:13` (`is_visible`, the Rust twin),
`crates/store/src/embed.rs:29,46,72` (expired rows never get an embedding, so the vector channel cannot find them).

## Design

- **No migration.** The mark already exists: `expires_at <= now()` is "expired since `expires_at`", and
  `expiry_noted_for` (0005) records that the owner was told. Migration numbering in the README is unchanged; if the
  implementer finds a column is truly needed, it is an escalation, and the next free number is `0012` (0007–0011 reserved).
- **Sweep** (`capture.rs`): delete the P1 statement. Keep the `lapsed` `UPDATE ... expiry_noted_for` and
  `note_expired` (the `memories_expired` audit event) as they are. Rename `SweepReport.memories_deleted` to
  `memories_expired` and fill it with `lapsed.len()` (memories newly marked expired this sweep, promoted copies included).
- **Core** (`kind.rs`): remove `EXPIRED_GRACE_SECONDS` (its only use is P1).
- **Reads**:
  - `visible` (`memory.rs:21`): drop the `expires_at` clause; it becomes identical to `visible_even_if_expired` —
    keep one function (`visible`) and point the callers of the other at it.
  - `searchable` (`memory.rs:35`): drop the `expires_at` clause.
  - `is_visible` (`access.rs:11`): drop the `expires_at` check; update its doc and the test at `access.rs:54`.
  - `embed.rs:29,46,72`: drop the `expires_at` clause so expired rows are (re)embedded.
  - The web/API `status=expired` filter (`memory.rs:339-352`, `routes/memory.rs:65,116`) keeps working; fix doc comments that say "not yet swept away".
- **How expired memories appear**: the `Card`/`Memory` reply already carries `expires_at`. The web shows an
  `expired <date>` badge on the memory table row and in `memory-sheet.tsx` when `expires_at <= now`, and the sheet text
  "This memory was forgotten or expired." (`memory-sheet.tsx:322`) becomes "forgotten" only. Search cards for agents
  add no new field (D-TOK): an expired card is identified by its existing `expires_at`. If cards today omit
  `expires_at`, the implementer adds the shortest marker the card format already uses for status and reports the bytes.
- **Ranking**: unchanged by this item (Open question 1).

## Allowed files

- `crates/store/src/capture.rs` (P1 removal, report field only)
- `crates/core/src/memory/kind.rs`
- `crates/store/src/memory.rs` (visibility helpers, doc comments only)
- `crates/store/src/stats.rs` (switch `visible_even_if_expired` import to `visible`)
- `crates/core/src/project/access.rs`
- `crates/store/src/embed.rs`
- `crates/server/src/routes/memory.rs` (doc comment)
- `apps/web/app/memories/_components/memory-table.tsx`, `apps/web/components/memory-sheet.tsx`
- `apps/web/lib/generated/*` by regeneration only
- tests: `crates/store/tests/capture.rs`, `crates/store/tests/retrieve.rs`, `crates/server/tests/retrieve.rs`, web unit test next to the table

## Acceptance checks

1. A memory with `expires_at = now() - 400 days` survives `sweep` with title, summary, embedding, entities and links unchanged; row count unchanged.
2. `sweep` reports `memories_expired` = memories newly past `expires_at`, and a second sweep reports 0 (no double count); one `memories_expired` audit event per owner as today.
3. An expired memory is returned by `memory_get`, by the web list (default view and `status=expired`), and by `memory_search` (lexical and vector channels) for a reader who could see it before it expired; a forgotten one is still not.
4. An expired row without an embedding gets one from the embed job.
5. The web shows `expired <date>` for it.
6. `grep -rn "DELETE FROM memories" crates/` returns only P2 (until Open question 2 is answered).
7. `python3 scripts/check_migrations.py` passes with no allowlist change (item 00 guard green).
8. Upgrade test (README global acceptance 9) holds: no memory row disappears across a sweep.

## Tests to write first

- `capture.rs::sweep_marks_expired_and_keeps_rows` (1, 2)
- `retrieve.rs::expired_memory_is_searchable_and_gettable` (3)
- `retrieve.rs::expired_memory_gets_embedded` (4)
- `access.rs` unit test updated: expired is visible, forgotten is not (3)
- web: `memory-table` renders the expired badge (5)

## Data-safety notes

Removes the only automatic hard delete of memory rows (P1). No schema change, no backfill. Rows already deleted by
0.1.8 cannot come back from code. Until 0.1.9 is deployed the live server keeps deleting memories 30 days after
expiry; an operator can stop it early by deploying this item alone.

## Token cost

Zero new fields. Search can now return expired memories, so a result list may include them in place of others
(same result count, same budget). Ranking decides how often (Open question 1).

## Risks

- Expired handoffs (14 days, merged branches) and 90-day notes/digests now show up in search and the handoff lookup
  could surface an old one. The latest-handoff lookup must keep choosing the newest live handoff; the implementer
  checks `kind=handoff` lookups prefer non-expired and reports if one needs a change (escalate, do not decide).
- A withdrawn promoted global copy (`consolidate.rs:334`) becomes visible to everyone as expired. It was meant to
  disappear; see Open question 3.

## Open questions

1. **Non-blocking (default: no ranking change).** Should expired memories rank lower in search (e.g. like `stale`)
   or be excluded from the hook's automatic injection while staying findable by explicit search?
2. **BLOCKING.** P2: when a new handoff replaces the old one for the same checkout (`capture.rs:427`), the old
   handoff row is deleted and re-inserted under the same id, losing the old text. Does the user's "never deleted"
   rule cover this? Options: (a) keep as is (handoff is a per-checkout slot); (b) keep the old handoff as its own
   memory with a new id, marked expired, and write the new one under the fixed id.
3. **Non-blocking (default: show it, marked expired).** Withdrawn promoted global copies: show them as expired to all
   readers like any other memory, or keep the existing "nobody's to see" exclusion (`memory.rs:348`) for them only?
