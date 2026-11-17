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

The three scopes are `project`, `personal` (private personal behaviour), and `global` (all server users), as defined in [the scope contract](project-layer4.md#memory-scopes-and-record). Agents choose scope explicitly; initialized-repository captures and summaries belong to the project. Branch/worktree/session are context fields for relevance, never additional ownership scopes. Personal behaviour and global notes can follow any checkout. Project selection in a scoped query uses `context.project_id`, never repository-name matching. Unfiltered list/search includes all project records visible through membership. Resume/unresolved retain account continuity.

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

## Hybrid recall (v0.1.4)

`search`, `smart-search`, `context`, `recall` and `why-changed` use the same
four-channel retrieval pipeline. Authorization, forgotten/expired/superseded
suppression and context filtering apply before fusion and link expansion.
Results retain `source: project | personal | global`; relevance never grants
visibility. Resume/unresolved keep their session continuity behavior.

- **Full text:** PostgreSQL uses its `simple` FTS index and `ts_rank_cd`;
  title is weight A and content weight B. The file store uses lexical substring
  counts with title weight 8 and other text weight 1. The existing tokenization
  differences remain; scores from these stores need not be identical.
- **Entity:** query terms and requested files/symbols/commands match stored
  `applies_to`, `evidence`, `tags`, `entities` and code context lists. An exact
  case-insensitive query or requested entity match scores 4; otherwise each
  matching query term scores 1. This selects relevance without changing scopes.
- **Semantic:** normalized 384-dimensional local MiniLM vectors, exact cosine
  similarity strictly above zero, at most 512 candidates after visibility and
  context filters. Zero is an inclusion rule, not a calibrated quality claim.
- **Links:** one hop over explicit memory IDs in `evidence` (either direction)
  or shared `(author, source_event_id)` pairs (capture IDs are person-scoped). Expand the first 512 fused visible seeds, rank
  neighbours by summed seed scores and retain at most 512. Newly discovered
  neighbours never become seeds in the same request. URL similarity is not a
  link. Hidden neighbours never become candidates or generated contradiction targets.

Each nonempty channel sorts descending by its score then ascending memory ID,
removes duplicate IDs and contributes `1 / (60 + rank + 1)` with zero-based
rank. Sum contributions, then multiply by type (`rule`/`decision`: 3,
other notes including session notes/handoffs: 2, `capture`: 1) and by 2 when
pinned. Title is the strongest lexical field and appears first, repeated three
times, in semantic input. These named policy constants are tunable in code;
changing them requires updated contract/tests. Final equal-score decision/contradiction tiebreaks
remain; equal raw lexical scores may have distinct fused ranks.

Retention and feedback multiply that fused score before the tiebreaks and
before limit truncation. `omitted_count` still counts every eligible hit.

```
score = fused * type_weight * pin_weight * retention_factor * feedback_factor
```

Salience is `lifecycle.importance / 100`. Age and recency are unix-second
elapsed time divided by 86400. Recency uses `last_accessed_at`, or
`created_at` when the memory has never been returned. `access_count` starts
at 0.

```
retention = 0.35 * salience
          + 0.25 * 0.5 ^ (age_days / 30)
          + 0.20 * (1 - 0.5 ^ (access_count / 5))
          + 0.20 * 0.5 ^ (recency_days / 14)
```

`retention_factor` is 1 when the kind does not decay or `retention` is at
least 0.45. Otherwise it is `0.25 + 0.75 * (retention / 0.45)`, so a
below-threshold memory ranks lower and stays in the result set. Nothing is
deleted or hidden because of this score. The temporary-observation expiry
sweep is a separate lifecycle and is unchanged.

Decay applies only to `capture`, `session_summary` and `handoff`. Rule,
decision, gotcha, procedure, fact, preference, explicit, lesson and correction
keep factor 1. Access stats still update.

```
feedback_factor = clamp(1 + 0.15 * (helpful_count - stale_count), 0.55, 1.45)
```

One `helpful` or `stale` vote per person; repeating the same vote does
nothing and the other value replaces it. Feedback does not change
`status`. Project votes from any member change ranking for every member.
Global votes from any authenticated user change ranking for everyone.
Personal votes are accepted only from the owner. A caller who cannot read
the memory gets the unknown-memory error.

Returned hits of `search`, `smart-search`, `context`, `recall` and
`why-changed` increment `access_count` and set `last_accessed_at`. Omitted
ids, `resume`, `unresolved`, the session brief and list/get do not. Both
stores persist `access_count`, `last_accessed_at`, the vote list and the
derived counts. These constants are tunable in `crates/core/src/retention.rs`;
changing them requires updated contract text and tests.

### Local model and background indexing

The server alone runs `fastembed` 7.7.1 with the FP32
`Qdrant/all-MiniLM-L6-v2-onnx` artifact, revision
`cb27c1f221563ca44e58ad2a694965db39c8153d`, mean pooling, 256-token inputs,
one CPU inference thread and batches of at most 8. UTF-8 input is bounded to
8192 characters before tokenization. The Apache-2.0 model and tokenizer files
are pinned by SHA-256 in `models/minilm.json`, verified during provisioning
and again at load. Production images bake them in. Standalone provisioning:

```sh
python3 scripts/download_embedding_model.py /path/to/minilm
cairn-server --store /path/to/store.json --embedding-model-dir /path/to/minilm
```

`CAIRN_EMBEDDING_MODEL_DIR` supplies the directory when the flag is absent.
There is no runtime model download or external embedding API. `/health` reports
`semantic_search: enabled` only after model load; missing/untrusted/unloadable
artifacts leave it `disabled` and lexical/entity/link retrieval remains available.
A failed or busy query inference also falls back for that request.

A background job starts immediately, then scans every 2 seconds for missing,
stale-input or stale-model vectors. It embeds at most 8 records per tick,
outside the file lock and async executor; captures/saves/searches never wait
for indexing to catch up. Query inference uses a nonblocking model lock.
Records not yet indexed have no semantic contribution. Failures log no memory
payload and retry on the next tick. A source fingerprint/model check before
commit rejects work racing an edit, correction, expiry or forgetting.

PostgreSQL deployments require the `vector` extension; the migration creates
it and persists `vector(384)` plus model ID/version, dimensions, normalization,
SHA-256 input fingerprint and creation time. A separate SHA-256 source-revision
check invalidates a changed database record immediately. The file store
persists equivalent vectors/metadata and verifies the current input fingerprint
before use. Both stores restart from persisted vectors; startup backfills
missing or stale ones. Derived vectors are not trusted client input, exported
memory or sync events. File job discovery scans local memory; PostgreSQL uses
a bounded pending-row query. This implementation uses exact vector search;
no approximate index or recall-quality benchmark is claimed.

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
then the fused order of `search` (score, then memory ID). The cut keeps the first
512 of that order, so a decision that ranks 513th by fused rank still leads at limit 1.
The same ranking runs in the file store. `resume` and `unresolved` are not capped,
and the file store has no cap.

## Session handoff retrieval

### Session-end distillation (v0.1.4)

Both stores atomically persist a `session_summary`, a separate `handoff`, and
up to 64 typed notes per session end. `POST session/end` still returns the
session summary; list/get exposes all records. Automatic idle close uses the
same path. No LLM or external model is used.

Every record has a title, summary, optional details, `evidence` and
`source_event_ids` linking retained captures, `applies_to` from touched files
and branch, and `work-note`/`distilled` tags. A handoff contains only the
extracted next steps and unresolved work (empty lists when none is stated).
An initialized project's notes are project-visible. Without an attached
project, or with mixed project/unattached sources, automatic work notes stay
private under `personal`; they are not personal behaviour preferences and
are never automatically global. Context filters still select these work notes
by repository, branch, worktree and session.

The typed grammar is deliberately explicit:

- A trimmed line beginning with `Decision:`, `Gotcha:`, `Procedure:`, `Rule:`
  or `Fact:` (case-insensitive) starts a note. Empty values are ignored.
  Its value is copied into summary and a bounded title; source lines are
  preserved in details. Ordinary prose alone creates no typed note.
- Immediately following labelled lines may fill `Reason:` for every type;
  `Decision context:`, `Alternative:`/`Alternatives:` and `Consequences:` for
  decisions; `Rule text:` and `Exceptions:` for rules; `Symptom:`, `Cause:`
  and `Fix:` for gotchas; `Step:` and `When:` for procedures; `Statement:`
  and `Source:` for facts. Each alternative or step line adds one array item
  without splitting or inference. Other lines end that note's metadata.
- Missing reason and per-type fields remain empty. A note type prefix alone
  does not populate its per-type fields, and causal language is not inferred.
  Agent saves still require title, summary and reason; this fallback does not.

Derived IDs are deterministic per person/session and source event/line, in
separate `handoff-` and `distilled-` namespaces. Typed IDs hash length-prefixed
session/source identifiers and the source line index to avoid ambiguous joins.
Re-ending an unchanged session preserves records and timestamps. Late captures
refresh the same session note/handoff and existing typed IDs; forgotten or
superseded records are never regenerated. Refresh keeps creation time,
verification and lifecycle state. Forgetting a project note hides only that
record; explicitly forgetting a source capture suppresses every derivation.
Private personal memory retains its owned-source cascade; see the
[lifecycle contract](lifecycle-v1.md#forgetting-shared-knowledge).
Typed extraction stops after 64 notes; all source IDs remain on the session
note, even when its bounded details omit text.

Ending a session deterministically extracts two structured lists from captured
observations:

- `next_steps` — observations containing explicit follow-up language such as
  `next step`, `follow up`, `should`, or `remaining`;
- `unresolved_work` — observations containing explicit uncertainty or incomplete
  work such as `not yet`, `blocked`, `unresolved`, `untested`, or `todo`.

The lists retain the normalized source line and source event ID, at most 16
items of 2048 characters per list. They are
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
- A trigger sweep is scoped to the person of the trigger. Resume and search never
  close a session. The file store sweeps on `capture_for_account`, device
  `sync/push` of a new observation, and `start_session_for_account`, not on the
  unscoped library call; PostgreSQL sweeps on device `sync/push` and account
  session start. A new device push on the file store also registers the project
  and branch like `capture_for_account`, and creates the session row (with the pushed context, on both stores) when the hook
  pushed without a session start, so a hook-only session is summarised by the sweep
  with the repository and branch scope. A sweep failure is logged and does not
  fail the capture or the start.
- A background sweep in `cairn-server` runs every 5 minutes (`IDLE_SWEEP_INTERVAL`
  in `crates/server/src/main.rs`) and applies the same idle rule to every person, so a
  session closes when the agent just stops, without new activity. The file store runs
  it under its store lock; PostgreSQL closes at most 16 sessions per run, oldest
  pending first, from the same partial index. The file store closes at most 16 per
  person per sweep or trigger. A failure is logged and never stops the loop; the
  next run continues. Entry points: `MemoryStore::sweep_idle_sessions`,
  `PostgresStore::sweep_idle_sessions`.
- PostgreSQL marks a session pending (`cairn_sessions.pending_since`) when a
  capture lands or one of its observations is forgotten, and clears it when the
  summary is derived. The sweep reads only that partial index, at most 16
  sessions per call, so its cost does not grow with finished sessions. A session
  whose close fails is marked pending again and retried after another idle
  period, not on every push.
- Automatic project registration validates the repository and branch against the
  same 512-byte limit as `register_project`; an oversized value registers nothing
  and does not fail the capture.

Implemented in `finish_idle_sessions` and `sweep_idle_sessions` (`crates/core/src/sessions.rs`,
`crates/storage/src/lib.rs`); the pending column is
`migrations/0012_session_summary_pending.sql`.

## Contradiction ordering

When two results have equal scores and list each other in `contradiction_ids`,
the memory with the later `created_at` ranks first. This only orders; nothing is
resolved or hidden. PostgreSQL applies it inside the capped window, so a tie that
the 512-row cap separates stays in memory ID order. It does not move a non-decision
ahead of a decision (see below).

## Decision-first ordering

Among results with equal scores, a memory with `kind` `decision` (a promoted
project decision or an explicit decision memory) ranks before every other memory in
`search`, `smart-search`, `context` and `recall`. The order of equal-score,
equal-kind results is then the contradiction rule above, then memory ID. Scores are
not changed, so a higher score still ranks first. Both adapters apply it before the PostgreSQL 512-row cap. `why-changed` keeps its own order (decision
reason, then relationships, then score). Implemented in `prefer_decision` and
`prefer_newer_contradiction` (`crates/core/src/recall.rs`) and `hybrid_matches`
(`crates/storage/src/embeddings.rs`).

## Why a change exists

The decision retrieval path combines hybrid recall with deterministic evidence
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

- `crates/core/src/sessions.rs` and `crates/core/src/recall.rs` implement scope derivation and filtered retrieval.
- `crates/server/src/lib.rs` exposes REST filters.
- `crates/client/src/tools.rs` carries the MCP and CLI filters to REST.
- `crates/core/src/sessions.rs` and `crates/core/src/recall.rs` extract session handoff fields and provide resume
  and unresolved retrieval.
- `crates/core/src/recall.rs` provides deterministic decision reasons, bounded code
  relationships, and stale/contradiction signals.
- `scripts/smoke_full_flow.sh` verifies branch-local memory does not leak into
  `main` context, that omitted results remain recoverable, and that handoff
  fields survive real REST retrieval.
- `scripts/measure_layer2.py` runs the Layer 2 golden-corpus checks against a
  real Cairn server process.

## Session-start brief (v0.1.4)

`POST /cairn-memory/session/brief` accepts `{"context":{...WorkContext...}}`
and returns `{"brief":"...", "max_bytes":4096}`. An account/device bearer is
required on both adapters, including the file backend before bootstrap. Reads
use the same authorized, retained memory set as memory listing: another person's
private notes and projects without membership never become candidates. The
endpoint does not join a project or initiate model inference.

Shared `cairn_core::session_brief` builds four independently budgeted sections:

1. Personal behaviour notes, excluding raw captures, session notes, handoffs and
   `work-note`-tagged automatic work notes.
2. The newest handoff matching the exact nonempty requested repository and
   branch, plus project ID (or both absent for private work). Missing repository
   or branch gives no handoff; other branches never supply one.
3. Rules/decisions in the requested project ID, with no branch or the matching
   branch. Missing project ID gives no project section.
4. Global notes, excluding raw captures/session notes/handoffs.

Only active records are included. Notes rank by pinned first, lifecycle
importance descending, creation timestamp descending, then stable ID. Handoffs
rank by newest creation then ID, regardless of pin. This is deterministic brief
selection, separate from query-driven hybrid recall. At most three notes per
section and one handoff are returned, under a 900-byte JSON budget per section.
Fields include ID, title, summary and reason; handoffs also carry bounded next
steps and unresolved work. Oversized records shrink to a compact ID/title/summary
preview; complete records remain accessible through memory tools.

`SESSION_BRIEF_MAX_BYTES=4096` bounds the complete UTF-8 text, including its data
label and JSON code fence. Field cuts respect UTF-8 boundaries and JSON escape
expansion. Backticks are escaped inside JSON so a stored string cannot terminate
the fence. The label explicitly identifies memory as evidence/data, never agent
instructions. No tokenization service or LLM is required. Every hook uses this
single route and the same selection function on both storage adapters. Hook
instruction and work-delivery limits live in [the hook contract](hook-v1.md#hook-output-and-work-tracking).
