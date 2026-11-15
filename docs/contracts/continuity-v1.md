---
id: CM-CONTRACT-006
title: "Layer 2 Branch-Aware Continuity"
status: draft
---

# Layer 2 Branch-Aware Continuity

This contract defines the first Layer 2 slice: capture coding work context and
filter recalled memories by the current session, worktree, branch, project, or
user-global scope.

## Memory scopes

| Scope | Meaning | Matching context |
| --- | --- | --- |
| `session` | A session-local memory or legacy record without safe broader context | Same `sessionId` only |
| `worktree` | An unmerged hypothesis tied to one repository, branch, and worktree | Repository + branch + worktree |
| `branch` | A branch-local fact that may apply across worktrees for that branch | Repository + branch |
| `project` | Repository-level project knowledge | Repository |
| `global` | A user-level memory without repository context | Any scoped query |

New session summaries derive their scope from their captured `WorkContext`:

```text
repository + branch + worktree -> worktree
repository + branch            -> branch
repository                     -> project
no repository                  -> session
```

Explicit memories use the same mapping, except a memory without repository
context is treated as `global`. This keeps personal guidance available across
projects while keeping branch/worktree hypotheses narrow.

## Retrieval filters

REST `/cairn-memory/search`, `/cairn-memory/context`, `/cairn-memory/recall`, and
`/cairn-memory/smart-search` accept optional filters:

```json
{
  "query": "cache decision",
  "limit": 5,
  "sessionId": "session-42",
  "context": {
    "repository": "cairn-demo",
    "branch": "feat/cache",
    "worktree": "wt-cache",
    "commit": "abc1234",
    "files": ["src/cache.rs"],
    "symbols": ["Cache::get"],
    "commands": ["cargo test cache"],
    "tests": ["cache_reads_are_consistent"],
    "services": ["cache"]
  }
}
```

The structured code context fields are additive and optional:

| Field | Meaning |
| --- | --- |
| `files` | Paths touched or relevant to the memory |
| `symbols` | Functions, methods, types, or other code symbols involved |
| `commands` | Commands run or recommended for verification |
| `tests` | Test names or test targets involved |
| `services` | Services, subsystems, or runtime components involved |

Session summaries merge these fields from their observations. Search indexes
them as lexical context, so a query such as `Cache::get` or `cargo test cache`
can recover the associated memory without requiring embeddings.

MCP query tools accept the same `sessionId` and `context` arguments. The CLI
accepts repository, branch, worktree, and commit after the optional limit for
`search`, `context`, `resume`, and `unresolved`.

When no filter is supplied, the existing Layer 1 unfiltered behavior remains
available for local administration and compatibility. When a filter is
supplied, a memory is included only if its scope matches the current context;
branch and worktree memory never matches an unrelated branch or worktree.

## Context pack response

Context packs preserve source event IDs and now include scope labels. If the
limit omits results, the response includes the omitted memory IDs and a bounded
recovery hint:

```json
{
  "query": "cache decision",
  "results": [],
  "omitted_count": 1,
  "omitted_memory_ids": ["memory-explicit-..."],
  "recovery_hint": "Increase the limit or query one of the omitted memory IDs to recover additional context."
}
```

PostgreSQL returns at most 512 matches per search (`MAX_SEARCH_RESULTS`). For
`context` and `why-changed`, `omitted_count` is every eligible match not
returned — same person, query, lifecycle, supersession, and filter predicate —
including matches past 512. `omitted_memory_ids` lists only omitted rows inside
that 512 window, so it can be shorter than `omitted_count`. When it is, the hint
says the results are capped and to narrow the query or filter, because a larger
limit cannot recover the rest. `why-changed` on PostgreSQL ranks every eligible
match before the 512 cut: decision reason present first, then relationship count,
then the lexical order of `search` (score, then memory ID). The cut keeps the first
512 of that order, so a decision that ranks 513th lexically still leads at limit 1.
The same ranking runs in the file store. `resume` and `unresolved` are not capped,
and the file store has no cap.

## Session handoff retrieval

Ending a session deterministically extracts two structured lists from captured
observations:

- `next_steps` — observations containing explicit follow-up language such as
  `next step`, `follow up`, `should`, or `remaining`;
- `unresolved_work` — observations containing explicit uncertainty or incomplete
  work such as `not yet`, `blocked`, `unresolved`, `untested`, or `todo`.

The lists retain the normalized observation text and source event ID. They are
returned on session-summary results and can be retrieved without a query:

| Surface | Resume | Unresolved work |
| --- | --- | --- |
| REST | `POST /cairn-memory/resume` | `POST /cairn-memory/unresolved` |
| MCP | `memory_resume` | `memory_unresolved` |
| CLI | `cairn resume` | `cairn unresolved` |

Both paths accept the same context filter and return a bounded context pack.
This is deterministic handoff extraction, not model-generated planning.

## Automatic session close

Sessions close without a human step (ADR 0009, automation-first). On the capture
of an account-owned observation and on a session start, both adapters finish the
same person's sessions through the ordinary end-session path, which derives the
summary memory:

- A session is finished when its newest retained observation (owned, not
  forgotten, not expired) is older than 30 minutes and it has no current summary.
  Only this idle rule closes a session. A session start does not close another
  open session, because a parallel agent of the same person on the same
  repository and branch must keep its own.
- A session with no retained observations is skipped, without an error.
- An ended session whose id received observations the summary does not cover (the
  agent reused the id), or whose summary has a forgotten source, gets a new
  summary: the next sweep, and an explicit end, derive it again from what is
  retained and replace the old one under the same memory id. The new summary keeps
  the earlier one's `verified_commit`, lifecycle (sensitivity can only rise) and
  `created_at`.
- A summary the person forgot, or that a live correction supersedes, is never
  derived again: the sweep leaves it alone, and an explicit end of that session
  returns a validation error instead of a hidden memory.
- The sweep is scoped to the person of the trigger. Resume and search never
  close a session. The file store sweeps on `capture_for_account` and
  `start_session_for_account`, not on the unscoped library call or device
  `sync/push` path; PostgreSQL sweeps on device `sync/push` and account session
  start. A sweep failure is logged and does not fail the capture or the start.
- PostgreSQL marks a session pending (`cairn_sessions.pending_since`) when a
  capture lands or one of its observations is forgotten, and clears it when the
  summary is derived. The sweep reads only that partial index, at most 16
  sessions per call, so its cost does not grow with finished sessions. A session
  whose close fails is marked pending again and retried after another idle
  period, not on every push.
- Automatic project registration validates the repository and branch against the
  same 512-byte limit as `register_project`; an oversized value registers nothing
  and does not fail the capture.

Implemented in `finish_idle_sessions` (`crates/core/src/lib.rs`,
`crates/storage/src/lib.rs`); the pending column is
`migrations/0012_session_summary_pending.sql`.

## Contradiction ordering

When two results have equal scores and list each other in `contradiction_ids`,
the memory with the later `created_at` ranks first. This only orders; nothing is
resolved or hidden. PostgreSQL applies it inside the capped window, so a tie that
the 512-row cap separates stays in memory ID order.

## Why a change exists

The decision retrieval path combines lexical recall with deterministic evidence
metadata:

| Surface | Path/tool |
| --- | --- |
| REST | `POST /cairn-memory/why-changed` |
| MCP | `memory_why_changed` |
| CLI | `cairn why-changed` |

Results include source event IDs, an extracted `decision_reason` when the text
contains `because` or `reason:`, structured code relationships, and signals for
stale or contradictory memories. The system exposes competing memories rather
than silently selecting a winner.

Relationships are deterministic and bounded. Current relationship types include
`memory_touches_file`, `memory_mentions_symbol`, `file_contains_symbol`,
`symbol_verified_by_test`, `command_verifies_test`, and `service_owns_file`.

Staleness is signaled when freshness is below 50, when a branch/worktree
memory in the same repository was captured at a different commit than the
current context, or when a project memory in the same repository has a
`verified_commit` that differs from the current commit. The commit checks need
a commit on both sides. A
contradiction signal is raised when same-scope memories share at least two
topic terms but have opposite explicit negation polarity. A contradiction
target must itself be eligible for recall.

A correction hides its predecessor in normal recall and in contradiction targets
only when the successor is not owned by a different person than the predecessor.
In the file store the owner is the account that owns the memory id. A successor
with no owner (an unscoped library call, or data from before accounts) counts. A
`supersedes` link from another account is stored and exported unchanged, never
rejected at import, and has no effect on this account's results. PostgreSQL
compares `person_id` in the query.

## Layer 2 measurement

Run the real-process golden-corpus measurement with:

```bash
python3 scripts/measure_layer2.py
```

The measurement reports first-informed-action success, repeated-investigation
stability, branch scope precision, and stale/contradiction signal coverage.

## Safety boundary

- Context relevance is not authorization.
- This local slice provides scope filtering, not identity or membership checks.
- Project promotion, cross-device synchronization, permissions, and shared
  project memory remain later layers.
- Missing context does not broaden a scoped query; worktree and branch records
  are excluded unless the required matching fields are present.

## Evidence

- `crates/core/src/lib.rs` implements scope derivation and filtered retrieval.
- `crates/server/src/lib.rs` exposes REST filters.
- `crates/client/src/tools.rs` carries the MCP and CLI filters to REST.
- `crates/core/src/lib.rs` extracts session handoff fields and provides resume
  and unresolved retrieval.
- `crates/core/src/lib.rs` provides deterministic decision reasons, bounded code
  relationships, and stale/contradiction signals.
- `scripts/smoke_full_flow.sh` verifies branch-local memory does not leak into
  `main` context, that omitted results remain recoverable, and that handoff
  fields survive real REST retrieval.
- `scripts/measure_layer2.py` runs the Layer 2 golden-corpus checks against a
  real Cairn server process.
