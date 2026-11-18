---
id: CM-ADR-0010
title: "ADR 0010 — Correction keeps linked history"
status: accepted
---

# ADR 0010 — Correction keeps linked history

## Context

A correction used to tombstone the target. The old record then dropped out of
normal recall because it was forgotten, and the replacement did not keep the
target's `project_id`.
[CM-RESEARCH-001](../research/memory-confidence-and-correction.md) compared how
other systems keep or replace a corrected record. That survey did not support
a below-75 confidence cutoff.

## Decision

1. `correct` inserts a durable `correction` memory. It does not call `forget`
   and it does not insert a tombstone. `supersedes` is the target id.
   `lifecycle.confidence` is `100`, including when the request supplied
   another confidence.
2. The old memory or observation stays in storage and in export. It is not
   added to `forgotten_ids`.
3. Normal recall skips a memory when another memory for the same person has
   `supersedes` set to its id and that newer memory is not forgotten and not
   expired. This applies to search, context, resume, unresolved work,
   why-changed, lessons, and project listing. Project listing keeps its
   same-project form of that filter.
   "Same person" is a rule on the successor, not on the link. In the file store
   a link counts unless the successor has an owner that differs from the
   target's owner, so a target with no owner is not hidden by an owned successor
   from another account. A successor with no owner (an unscoped library capture, or
   data from before accounts) still counts. The link is stored and exported
   unchanged. It is inert, not rejected, so an export with an unknown target
   stays importable, and it takes effect again only for a successor of the same
   owner. PostgreSQL already compares `person_id` in its query.
4. When the target is a memory with a `project_id`, the correction copies
   that `project_id`. Observations have no `project_id`. Context and scope
   inheritance stay as they are.
5. Forget still tombstones. Export, diagnose counts, audit, and promote stay
   otherwise unchanged. Correction does not retire project entities, add a
   relation table, or apply a below-75 cutoff.

## Consequences

- The old record remains available in storage and export. Normal recall
  returns the correction and does not report the predecessor as an active
  contradiction while a live successor exists.
- Forgetting or expiring the correction makes the older memory eligible for
  normal recall again, because the skip requires a live successor.
- A later graph edge can build on the stored `supersedes` link. This decision
  does not add that table.

## Evidence

- `crates/core/src/memories.rs`
- `crates/storage/src/lib.rs`
- `crates/server/src/lib.rs`
- `crates/storage/tests/postgres.rs`
- `docs/contracts/lifecycle-v1.md`
