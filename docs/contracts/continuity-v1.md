---
id: CM-CONTRACT-006
title: "Layer 2 Branch-Aware Continuity"
status: draft
---

# Layer 2 Branch-Aware Continuity

This contract describes retrieval and session continuity in the 0.1.6 server: scopes and filters, hybrid recall,
the relevance floor, session handoffs, background jobs, consolidation, links, the session brief and save
identity. The design source is the [0.1.6 foundation specification](../specs/0.1.6-foundation.md). The
0.1.5 text (similarity reviews, lifecycle fields, session summaries, the file store, import) is retired with its
code.

## Memory scopes

The three scopes are `project`, `personal` and `global`, defined in
[the scope contract](project-layer4.md#memory-scopes-and-record). Branch, worktree and session are context
fields for relevance, never ownership. Project selection in a filtered query uses `context.project_id`, never
repository-name matching.

## Retrieval filters

`/search` takes this body:

```json
{"query": "cache decision", "kind": "decision", "limit": 5,
 "context": {"repository": "cairn-demo", "branch": "feat/cache", "worktree": "wt-cache", "project_id": "project-…"}}
```

`kind` keeps one kind (`kind=handoff` replaces the old resume route). With no `context`, nothing else is narrowed.
Otherwise a global memory always matches; a project memory matches only the context's `project_id` (inside git
without `.cairn/project.json` there is none, so project memories are hidden until `cairn init`); a personal
memory matches, except that a handoff must also match the repository, branch and worktree that are given. The
reply is `{results, more}`; `more` is the number of hits left out by the limit.

## Hybrid recall (v0.1.4)

Three channels run over the rows visible to the caller, each returning at most 512 candidates:

| Channel | Source |
| --- | --- |
| Lexical | `websearch_to_tsquery('english', q)` over the weighted `search` column (title A, summary B, reason and fields C) |
| Entity | the query's tokens against `entities.name` (files, symbols, commands, tests, services, terms) |
| Vector | cosine distance on the HNSW index, using the 384-dimension MiniLM embedding of the query; absent when no model is loaded |

Channels are fused by reciprocal rank (k = 60), then one-hop link neighbours of the top hits are added
(`supersedes` and `contradicts` are not hops). Scores are multiplied by a kind weight (rule and decision 3.0,
handoff 1.0, every other kind 2.0) and by a feedback multiplier: each `helpful` vote adds 0.15 and each `wrong`
vote subtracts 0.15, capped at ±0.45; fetching never raises a score. Votes count distinct people. A memory goes `stale` after `wrong` votes from two people,
or one from its owner; a stale memory is still returned, with `status:"stale"` on its card; superseded and forgotten memories are not returned. The vector channel takes its top candidates before the visibility join; nothing from another person
leaves the store, because the rule is applied in the same query before ids are returned.

### Relevance floor

A hit must show evidence of its own: an exact entity name, a technical term (containing `_` or a digit), a
distinctive word in the title, a distinctive word in the text of a rule or decision, or at least two distinctive
words (or one of five or more letters, for kinds other than note and handoff). A hit that shares no word with the
query is dropped even when the vector channel found it, so paraphrase queries can miss; step 5 of the rewrite
changes this.

### Local model and background indexing

The server loads the pinned MiniLM model from `--embedding-model-dir` or `CAIRN_EMBEDDING_MODEL_DIR`
(provisioned by `scripts/download_embedding_model.py`; digests in `crates/server/assets/minilm/minilm.json`).
Without it, lexical, entity and link recall work and semantic search is off. Embedding text is the title three
times, the summary, the reason, the field text and the entity names; a vector is stored with the hash of that
text and replaced when the text changes.

## Session handoff retrieval

### Session-end distillation (v0.1.4)

`POST session/end` and `session/checkpoint` enqueue a `distill` job, and the worker writes, with no model: one
`handoff` per (owner, session), upserted under a derived id, and up to 64 `note` memories from lines that begin
`Decision:`, `Gotcha:`, `Procedure:`, `Rule:` or `Fact:` (case-insensitive). The handoff carries next steps, open
questions and the files touched. Output goes through the same validation and redaction as an agent's save, with
origin `system`. A handoff is personal, expires after 14 days, and is project-attached when the session was; a
note expires after 90 days. Prose without a marker creates no typed note. Re-ending an unchanged session changes
nothing.

### Empty-session close (v0.1.6)

A session with no retained observations (none captured, or all forgotten or expired) has nothing to distill.
`POST session/end` answers `{"status":"empty"}`, writes nothing and leaves `ended_at` unset; otherwise it answers
`{"status":"queued"}`. A checkpoint answers `stored` or `unchanged`. The idle sweep follows the same rule: a
session whose last observation is more than 30 minutes old is closed and distilled, and a newer capture sets the
pending marker again.

### Durable jobs (v0.1.4)

One `jobs` table holds `embed`, `distill` and `consolidate` jobs, keyed so a job is enqueued once. A worker claims
a job with a 120-second lease; five failed attempts quarantine it (status `quarantined`) after backoffs of 30, 60, 120 and
240 seconds, and permanent errors (a vector that is not a finite normalized 384-vector, for example) quarantine it
at once. An admin can requeue or discard a quarantined job from the console (`GET /admin/jobs`,
`POST /admin/jobs/{id}/requeue`, `POST /admin/jobs/{id}/discard`). Finished jobs are deleted after a day. The
sweep also deletes expired memories 30 days after their expiry and expired observations.

### Consolidation

After a memory is embedded, `consolidate` compares it with its 20 nearest neighbours that have the same owner,
scope, project and kind. Cosine ≥ 0.92: the newer memory supersedes the older, which becomes `superseded` with
its evidence unioned onto the newer and a `supersedes` link of origin `auto`. Cosine from 0.75 to 0.92: a
`relates` link of origin `auto`. Lower: nothing. A declared `contradicts` link is never overridden.

## Context pack response

`{results, more}` where each card has the memory's `id`, `kind`, `title`, `summary`, `score` and, only when the
memory is stale, `status`. `POST /get` returns the full records (`reason`, `fields`, `evidence`, `links`).

## Declared contradictions and links (v0.1.4)

A save may name `supersedes` and `contradicts` (one id each); there are no separate link routes. Relations are `relates`, `causes`, `fixes`, `depends_on`, `supersedes` and
`contradicts`. Both ends must be visible to the caller. `GET /projects/{id}/conflicts` lists live project
memories joined by `contradicts`. `GET /graph` returns visible memories
as nodes (`memory_id`) and links as edges (`source_id`, `target_id`) with an optional scope, project, relation and
node cap; the console page `/graph` draws it.

## Why a change exists

`POST /search` with `kind=decision`, then `POST /get`, returns the reason, evidence and links, so a reader can see
what replaced or contradicts what.

## Session-start brief (v0.1.4)

`POST /session/brief` returns text of at most 4,096 bytes, fenced as data: the latest matching handoff, up to
three project rules and decisions for the context's project, and up to two personal preferences, each section
bounded, ordered by feedback multiplier, newest and id. Only active memories are included; global memories are
not. The hook injects it at session start.

## Explicit-save identity (v0.1.6)

A save reuses one record when the same owner supplies the same kind, title, summary, reason and fields after
redaction (`identity_hash`). Evidence is excluded from the comparison and merged, so a retry may add evidence;
a retry with none is a no-op. A different text, kind, owner or scope is a separate record. A forgotten identity
is never reused: the save returns `409 forgotten`. The receipt is `{id, status, deduplicated}`.

## Evidence

- Core: `crates/core/src/retrieve/{filter,hybrid,rank,pack,brief,graph}.rs`,
  `crates/core/src/process/{distill,consolidate,jobs,embed}.rs`.
- Store: `crates/store/src/{retrieve,consolidate,embed,jobs,capture}.rs` with tests in `crates/store/tests/`
  (`retrieve.rs` includes the `EXPLAIN` checks for the three indexes).
- Server: `crates/server/src/{routes/retrieve.rs,worker.rs}`, tests `crates/server/tests/{retrieve,semantic}.rs`.
- Quality: `scripts/bench.py` (hit@5, MRR, zero-hit, response sizes); the reference runs are in `bench/`.
