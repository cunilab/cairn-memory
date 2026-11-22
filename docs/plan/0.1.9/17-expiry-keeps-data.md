---
id: CM-PLAN-019-17
title: "17 Always-active memory: no expiry, relations and ranking"
status: frozen
---

# 17 Always-active memory: no expiry, relations and ranking

## Goal

User decision 2026-11-23 (binding). A memory is never deleted and never hidden because of its age. `expires_at`/TTL
no longer hides or deletes anything. Old versus new is handled by **relations and ranking**: a newer memory related
to an older one ranks above it; the older one stays findable, ranked lower, with a short label saying why.

| State | In search / `memory_get` | Rank | Label on the card |
|---|---|---|---|
| active | yes | normal | — |
| superseded / corrected | yes | below its replacement | `replaced by <new id>` |
| stale by votes | yes | lower | `voted wrong <n>x` |
| stale, file missing | yes | lower | `file deleted` |
| withdrawn promoted global copy | yes, stays `active` | by score | — (related to its originals) |
| old handoff | yes | lower (superseded) | `replaced by <new id>` |
| **forgotten** | **no** (not in search, not in the brief); admin sees it in the web | — | — |

`forgotten` is the only hidden state. Raw logs keep their retention: `observations`, done `jobs`, `retrieval_log`,
item 13's 14-day search log.

## Out of scope

Restoring rows already deleted by 0.1.8 (gone; backup restore is an operator task). New relation kinds (the existing
`supersedes` and `relates` cover every case). Changing result count or token budget. Internal processing cohorts
(`link.rs:27`, `consolidate.rs:101,218,240`, `repo.rs:109,188`) other than dropping their `expires_at` clauses.

## Decisions it relies on

D-DATA, D-TOK, D-LLM, D-RANK, the 2026-11-23 decision above.

## Paths found (branch `0.1.9` at `02e7d90`)

| # | file:line | Today | This item |
|---|---|---|---|
| P1 | `crates/store/src/capture.rs:513-518` | Nightly `DELETE FROM memories WHERE expires_at < now() - 30 days`; FK cascade drops usage, entities, links, votes. **Live data loss.** | Removed |
| P2 | `crates/store/src/capture.rs:376-427` | Handoff has a fixed id per checkout (`handoff_id`); a new handoff `DELETE`s and re-inserts it, losing the old text. | Becomes a chain (see Design) |
| P3 | `crates/store/src/memory.rs:562-609` (`forget`) | Sets `forgotten`, blanks text/fields/embedding, deletes entities, links, jobs. | Only sets `forgotten`; keeps everything |
| P4 | `memory.rs:508` | Replaces a person's own vote row. | Unchanged |
| P5 | `capture.rs:531,537,545,551` | Raw log retention. | Unchanged |
| P6 | `jobs.rs:136,215,303`, `instructions.rs:244` | Job / server_state cleanup, no memory content. | Unchanged |

Writers of `expires_at`: `kind.rs:14` TTLs, `capture.rs:524-533` (unused system memory 90 days),
`consolidate.rs:334` (withdrawn promoted copy), `repo.rs:285` (merged-branch handoffs). Readers that hide by expiry:
`memory.rs:23` (`visible`), `memory.rs:37` (`searchable`), `access.rs:13` (`is_visible`), `embed.rs:29,46,72`,
`consolidate.rs:330` (live-source count).

## Design

- **Expiry off.** Delete P1 and `EXPIRED_GRACE_SECONDS`. New memories get `expires_at = NULL` (TTL constants for
  memory kinds removed from `kind.rs`); the four writers above stop setting it. Existing values stay in the column
  (no data change) and are ignored by every reader. The `lapsed`/`expiry_noted_for`/`memories_expired` audit step is
  removed from the sweep (nothing expires); the column `expiry_noted_for` stays. Web: drop the `status=expired`
  filter and the "or expired" sheet text.
- **Searchable set.** `searchable` becomes `status IN ('active','stale','superseded')` with no `expires_at` clause;
  `visible` / `is_visible` drop the `expires_at` clause (forgotten stays excluded for non-admins); `embed.rs` embeds
  every non-forgotten row. One shared helper, not per-call SQL.
- **Ranking** (`rank.rs::adjust`, the one place): `Status::Superseded` → `-0.30` (the cap), `Stale` stays `-0.20`.
  A replacement is always `active`/`stale` and therefore ranks above the version it replaced for an equal fused score.
  Results stay ≤ 5 and within the existing budget: old versions compete for the same slots. The weak/off-topic gate
  runs on the fused score **before** `adjust` as today, so it is unchanged; verify with the bench.
- **Labels** (agent cards and web): derived at read time, no new column.
  `replaced by <id>` from the newest `supersedes` link pointing at the memory; `voted wrong <n>x` from
  `stale_cause = 'feedback'` and `usage.wrong`; `file deleted` from `stale_cause = 'missing'`. Emitted only on those
  cards, in the card's existing status slot. Token cost: ≤ 42 bytes (`replaced by mem_` + 26-char id) on a
  superseded card, ≤ 16 bytes on a stale card, 0 on active cards. The web shows the same label with a link to the new id.
- **Withdrawn promoted global copies** (`consolidate.rs:334`): no longer set `expires_at`; they stay `active`, keep
  their existing `relates` links to the originals (already written by promotion), and rank by score.
- **Handoff chain (P2).** Each handoff is a new memory with its own id (no more `DELETE` + re-insert). The writer
  links it to the previous handoff of the same checkout (same person + worktree/branch, the `handoff_id` key becomes
  the chain key) with a `supersedes` link (newer → older) and marks the older `superseded`. The session brief and
  `kind=handoff` lookup return only the newest non-forgotten handoff of the chain. The web shows the chain from the
  links. `repo.rs:285` (merged branches) no longer expires them.
- **Forget (P3).** Only `status = 'forgotten'`, `status_changed_at`, audit event. Text, fields, embedding, entities
  and links are kept; pending jobs for it are cancelled as today. Hidden from search, brief and non-admin web; the
  admin sees it in the web memory list (`status=forgotten` filter). `restore_if_unsuperseded` behaviour unchanged.
  The `identity_hash` rule (the same content stays forgotten) is unchanged.
- **Migration `0012_always_active`** (index only, additive): the vector index of `0003` is partial on
  `status IN ('active','stale')`, so superseded rows would force a scan. Add
  `CREATE INDEX ... ON memories USING hnsw (embedding ...) WHERE embedding IS NOT NULL AND status IN ('active','stale','superseded')`
  (same opclass/params as 0003; the implementer copies them) and the matching lexical partial index if one exists.
  The old indexes are kept (item 00 forbids DROP). No backfill: existing rows already have the statuses and links.
  Old superseded rows without an embedding get one from the embed job.

## Interactions

- **01 (candidate rank):** `candidate -0.10` stacks with `superseded -0.30` under the ±30 % cap; a superseded
  candidate sits at the floor, same as a superseded normal. No change to 01.
- **04 (digest):** built with `expires_at = created + 90 days` and test `digest_is_project_candidate_with_note_ttl`.
  After 17 the digest never expires; the item-17 implementer updates that test to expect `expires_at IS NULL` and
  the digest writer in `distill.rs`/`capture.rs`. 04 gets a one-line "superseded by 17" note only.
- **05 (kept-richer handoff):** the chain replaces "keep the old row and skip the write". Default (non-blocking Q1):
  a thin new handoff is still written into the chain, and the brief shows the newest handoff that is not thin when
  the newest is thin (05's richer rule moves from write to read); `DistillReport.handoff_kept` keeps its meaning.
- **11 (merge veto):** veto yields `relates` instead of `supersedes`, so both stay `active` with no label — correct.
  `proof_count` backfill counts `supersedes` links with `origin='auto'`; handoff chain links use `origin='system'`
  so they are not counted.
- **13–16 (Fixes):** F12 `fade` is already rank-only (`faded_at` penalty, cleared on fetch); it must never hide —
  confirmed, no change. F7 `split_merge` Apply deletes one auto `supersedes` link and calls
  `restore_if_unsuperseded`: the older memory returns to `active` and its `replaced by` label disappears; this is a
  link removal by an admin, not memory data loss. 13's 14-day log expiry is a raw log, allowed.

## Allowed files

- `crates/store/src/capture.rs` (P1, sweep audit step, handoff chain, digest TTL)
- `crates/core/src/memory/kind.rs`, `crates/core/src/process/distill.rs` (handoff/digest TTL only)
- `crates/store/src/memory.rs` (visibility helpers, forget, status filter)
- `crates/store/src/stats.rs`, `crates/store/src/embed.rs`, `crates/store/src/consolidate.rs` (withdrawn copy, live count clause), `crates/store/src/repo.rs` (merged-branch handoffs)
- `crates/store/src/retrieve.rs` (status set, labels, latest-handoff lookup)
- `crates/core/src/project/access.rs`, `crates/core/src/retrieve/rank.rs` (Superseded weight only)
- `crates/server/src/routes/memory.rs`, card formatting where the status slot is rendered (implementer names the file; escalate if outside this list)
- `migrations/0012_always_active.sql`, `crates/store/src/lib.rs` (migration list entry)
- `apps/web/app/memories/_components/memory-table.tsx`, `apps/web/components/memory-sheet.tsx`, `apps/web/lib/generated/*` by regeneration
- tests: `crates/store/tests/{capture,retrieve,processing,migrate}.rs`, `crates/server/tests/retrieve.rs`, web unit test next to the table

## Acceptance checks

1. A memory with `expires_at = now() - 400 days` survives `sweep` unchanged and is returned by `memory_get`, web list and `memory_search` (lexical and vector).
2. New memories of every kind are written with `expires_at IS NULL`; `grep -rn "DELETE FROM memories" crates/` returns nothing.
3. Superseding A by B: both are searchable; for a query matching both equally B ranks above A; A's card carries `replaced by <B id>` (≤ 40 bytes).
4. A memory stale by 2 wrong votes is searchable with `voted wrong 2x`; stale-missing with `file deleted`; both rank below an equal active memory.
5. Three handoffs on one checkout give three rows linked newer→older by `supersedes`; the brief and `kind=handoff` return only the newest (or per Q1 default); the web shows the chain.
6. `memory_forget` keeps title, summary, fields, embedding, entities and links; the memory is absent from search and the brief and present for the admin with `status=forgotten`.
7. A withdrawn promoted global copy stays `active` and keeps its `relates` links.
8. Bench: weak queries 14/14 return nothing; v2 hit@5 ≥ CI floor (0.65); v1 floors hold. Result count ≤ 5 and the search reply byte budget unchanged.
9. `python3 scripts/check_migrations.py` passes with no allowlist change; upgrade test keeps every row count and `memory_id`.

## Tests to write first

- `capture.rs::sweep_never_deletes_or_expires_memories` (1, 2)
- `retrieve.rs::superseded_is_searchable_ranked_below_and_labelled` (3), `::stale_labels` (4)
- `processing.rs::handoffs_form_a_chain_and_brief_shows_latest` (5)
- `retrieve.rs::forget_keeps_content_but_hides` (6)
- `capture.rs::withdrawn_promoted_copy_stays_active` (7)
- `migrate.rs::0012_is_additive_on_populated_db` (9); `rank.rs` unit test for the Superseded weight; web label test

## Data-safety notes

Removes every automatic delete of memory rows (P1, P2) and the content blanking of forget (P3). Migration 0012 adds
indexes only. Until 0.1.9 deploys, the live server keeps deleting 30 days after expiry; deploying this item alone
stops it.

## Token cost

0 bytes on active cards; ≤ 16 bytes on stale cards; ≤ 40 bytes on superseded cards. Same result count and budget.

## Risks

- Superseded rows now compete for slots and could push out an active memory with a lower fused score; the −0.30
  weight and the bench check (8) bound this. If v2 hit@5 drops below the floor, escalate (do not change weights freely).
- More rows embedded (old superseded/expired): one-time embed backlog on upgrade.

## Open questions

1. **Non-blocking (default above).** Item 05's richer-handoff rule: apply at read (brief skips a thin newest
   handoff for the previous richer one) or drop it (brief always shows the newest)?
