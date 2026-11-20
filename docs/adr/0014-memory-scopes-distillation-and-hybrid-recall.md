---
id: CM-ADR-0014
title: "Memory scopes, distillation and hybrid recall"
status: proposed
---

# Memory scopes, distillation and hybrid recall

Date: 2026-11-17.

## Context

Fact research of two open memory systems, akitaonrails/ai-memory and mempalace,
showed practices Cairn lacks: automatic distillation of sessions into typed
notes, hybrid ranking, a bounded session-start brief, usage-based retention and
capture hygiene. Cairn's current split into project, personal-per-repo and
personal-global memory, plus an explicit promote step, makes members do manual
work that the product direction (minimal human interaction) rejects.

Items 0 through 10 are implemented on the v0.1.4 branch. The scope and record changes supersede the scope, personal-memory and
promotion parts of [ADR 0013](./0013-shared-project-decisions-and-personal-memory-scopes.md)
(decisions 1, 2 and 5 there, and the `source: personal | project` labelling in
decision 7). The rest of ADR 0013 remains. Pre-release rules still apply: no
data migration or legacy repair. v0.1.4 uses no LLM and adds no benchmark.

Already implemented in the v0.1.4 branch and unchanged by this ADR:
`.cairn/project.json` carries the project name, `cairn init --help` is resolved
locally, and only the owner can rename (`cairn init --rename <name>`). See the
2026-11-17 amendment in ADR 0013.

## Decision

The owner decided the following.

0. **Three memory scopes** replace project, personal-per-repo and
   personal-global:
   - *Project memory*: rules, decisions, gotchas, procedures, facts and raw
     captures (prompts, tool results, session summaries) from repositories
     initialized with `cairn init`. Visible to all project members.
   - *Personal behaviour memory*: how the agent should answer the person,
     preferences and habits. Private to that person and never shared.
   - *Global memory*: design patterns and important lessons, with the same note
     types. Visible to everyone on the server. Default edit and delete rights:
     author and administrator.

   The agent chooses the scope explicitly when saving, through an MCP/CLI
   parameter. Captures in an initialized repository go to project memory. The
   old personal per-repo scope and the promote step are removed.
   Consequence: raw captures are visible to all project members and global
   notes to all server users. Redaction still applies before storage.
1. **Rule-based distillation at session end**, without an LLM: a session note,
   a handoff and typed notes (decision, gotcha, procedure, rule, fact), each
   linked to its source captures. In an initialized repository they go
   straight into project memory.
2. **Hybrid recall** combining full-text, entity, links and local vector
   embeddings. Embeddings come from a small local model (such as MiniLM) run
   inside `cairn-server`; no external API is called. Rankings are fused by
   reciprocal-rank fusion, `1 / (60 + rank + 1)`, with type weights
   (rule/decision above notes above raw captures) and a pinned boost. Results
   are labelled project, personal or global.
3. **Session-start brief** injected by the hook, bounded in size and fenced as
   data, not instructions: personal behaviour memory, the last handoff, top
   project rules and decisions, and top global notes.
4. **Retention score** with reinforcement on use. A memory below the threshold
   is ranked lower only and is never auto-deleted. Rules and decisions do not
   decay.
5. **Capture hygiene**: assistant text is not captured by default (opt-in); a
   marker file skips capture per repository or folder; secrets are scrubbed
   before truncation.
6. **Feedback** helpful/stale on any memory. Any member's feedback on project
   memory changes ranking for everyone. For global memory any user may give
   feedback and it affects everyone.

## Memory record shape (decided 2026-11-17)

Decision 7. A memory has common fields plus per-type fields.

**Common fields**

- `title`: short; highest recall weight.
- `summary`: 1-3 sentences, the what.
- `details`: optional body.
- `reason`: the why.
- `applies_to`: paths/globs, branch, language/tool; used to select memories for
  the files being worked on.
- `evidence`: files, symbols, commands, commit, links to source captures.
- `tags` / entities.
- `status`: active, stale or superseded, plus `supersedes`.
- `pinned` / importance.
- author, created, changed.

**Per-type fields**

| Type | Fields |
|---|---|
| decision | context, alternatives considered, consequences |
| rule | rule, exceptions |
| gotcha | symptom, cause, fix |
| procedure | ordered steps, when to use |
| fact | statement, source |
| preference (personal behaviour) | preference, example |

**Required** when an agent saves via MCP/CLI: `title`, `summary`, `reason`.
Everything else is optional.

**Constraint.** Rule-based distillation (no LLM) can reliably fill only
`title`, `evidence`, `applies_to` and `tags`. `reason`, alternatives, cause and
fix come from the agent. The fallback may copy explicitly labelled source
summary/details and fields verbatim; it never infers missing causal or typed
fields. The [continuity contract](../contracts/continuity-v1.md#session-end-distillation-v014)
defines the bounded grammar.

### Stage 2 contract decisions (2026-11-17)

- Session end keeps the `session_summary` response and stores a separate
  `handoff` plus typed notes. Automatic notes without an attached project,
  including mixed project/unattached sessions, remain private in `personal`,
  tagged `work-note`; they never become global automatically.
- The hook opt-in is `CAIRN_CAPTURE_ASSISTANT=1`. A regular `.cairnignore`
  file skips automatic capture. Lookup starts at resolved payload `cwd`, or
  process cwd, and stops at the inclusive Git toplevel inside Git, otherwise
  at filesystem root. Secret scrubbing precedes every content cut and the
  generated fingerprint. Exact behavior lives in the
  [hook contract](../contracts/hook-v1.md#capture-hygiene-v014).
- Forgetting a project note hides that record only, preserving shared captures
  and sibling notes on both stores. Explicit source-event forgetting suppresses
  every derivation. This implements the owner's rule that hiding one item
  must not silently remove other shared knowledge; see the
  [lifecycle contract](../archive/contracts/lifecycle-v1.md#forgetting-shared-knowledge).

### Stage 3 contract decisions (2026-11-17)

The owner approved `fastembed` 7.7.1 with minimal CPU ONNX features and the
SHA-verified FP32 MiniLM artifact pinned in `crates/server/assets/minilm/minilm.json` (Apache-2.0).
The production image bakes it in; standalone servers load a provisioned directory.
Runtime model downloads/external embedding APIs are disabled. Model load failure
preserves lexical/entity/link recall and reports semantic search disabled.

PostgreSQL stores `vector(384)` with model/input provenance; deployments require
pgvector. The file adapter persists equivalent vectors and metadata. A bounded
background job performs startup backfill and updates after writes, with optimistic
fingerprint checks before publishing. Search never initiates a re-embedding job.
The exact approved channel definitions, RRF/type/pinned constants, 512-candidate
bounds and fallback behavior are owned by the
[continuity contract](../contracts/continuity-v1.md#hybrid-recall-v014).
Resume/unresolved and PostgreSQL omission/why-changed cap semantics remain.

## Agent instructions and agent-written memories (decided 2026-11-17)

Decision 8.

- Setup installs agent guidance alongside capture and MCP so agents know
  when and how to save.
- **Agents with hooks** (e.g. Claude Code SessionStart/PostToolUse/Stop):
  instructions are injected by hook.
  - Session start: a short note (1-2 lines: Cairn exists, use its memory tools).
  - First real work in the session (first file edit or command): the full but
    compact instructions, once: three scopes, required fields, per-type
    fields, when to save.
  - Both injections are token-bounded.
- **Agents using instruction-file fallback** (e.g. this OpenCode adapter): the instructions go in the first
  file the agent reads (`AGENTS.md` or the agent's equivalent).
- **Session end**: only if the session had real work (edited files or ran
  commands), the hook asks the agent to save what is worth remembering
  (decisions, rules, gotchas, procedures, preferences) with title, summary,
  reason, scope and per-type fields. No server-side LLM.
- Rule-based distillation stays as fallback (session note, handoff, evidence,
  touched files).
- Implementation must verify, per agent (Claude Code, Codex, OpenCode and any
  other agent `cairn setup` supports), from official docs which hook events
  exist and whether they can inject context or ask the agent to continue.
  Unsupported means instruction-file fallback. No assumptions.

The record shape is a storage and public-contract change; it lands with the
checklist items below (pre-release, no data compatibility).

### Stage 4 contract decisions (2026-11-17)

The owner approved explicit `cairn setup <url> --agent claude|codex|opencode`
and `--remove`, including MCP registration and only marked integration changes.
URL-only setup retains its behavior. Explicit agent setup uses the matching
binary's bundled scripts/instruction text; updates verify release assets as usual.

Official Claude Code and Codex docs support start/post-tool context and Stop
continuation; SessionEnd cannot prompt continuation. Therefore Stop asks after
new work since the preceding ask, with `stop_hook_active` recursion suppression;
SessionEnd retains rule-based distillation. OpenCode V2 has plugin hooks, but
Cairn uses the approved global instruction-file fallback without a plugin or
shell continuation protocol. No automatic brief/first-work/Stop injection is
claimed for OpenCode. Exact capabilities and official citations live in the
[hook contract](../contracts/hook-v1.md#verified-agent-capabilities).

Native Codex/OpenCode MCP commands preserve TOML/JSONC configuration. Since the
OpenCode CLI has no removal command, the owner approved a tested JSONC span
scanner for ownership checks and deletion of only the Cairn-marked entry, with
all unrelated bytes/comments preserved. Missing CLIs and unmarked name conflicts
fail safely; no parser dependency or Python version increase is required.

The authenticated brief route is shared by both adapters; deterministic section
selection, field truncation and the 4096-byte data fence are defined by the
[continuity contract](../contracts/continuity-v1.md#session-start-brief-v014).
All agent scope/field guidance comes from one bounded instruction source. No
LLM or new recall-ranking signal is added by stage 4.

### Stage 5 contract decisions (2026-11-17)

The owner approved the retention and feedback multipliers in the
[continuity contract](../contracts/continuity-v1.md#hybrid-recall-v014). They
multiply the stage-3 fused score before tiebreaks and truncation. A memory
below the retention threshold ranks lower only. Rules, decisions, gotchas,
procedures, facts, preferences and other purposeful saves (explicit, lesson,
correction) do not decay. Captures, automatic session summaries and handoffs
do. Helpful or stale feedback is one vote per person on any visible memory:
project members affect every member, any authenticated user affects global
ranking, and only the owner can vote on personal memory. Returned hits of
search, context and why-changed reinforce access stats. Resume, unresolved,
the session brief and list/get do not.

Feedback uses the ordinary read gate, including forgotten sources, expiry,
record status and live successors. PostgreSQL locks the memory before reading
and replacing its votes so concurrent voters cannot overwrite each other.
Summary regeneration locks and carries the same memory's reinforcement and
feedback state into the replacement. Imports clear votes and recompute their
counts in both stores: a backup cannot authenticate a voter. The
[export contract](../archive/contracts/export-v1.md#import-behavior) owns that rule.

### Stage 6 contract decisions (2026-11-17)

The owner approved one durable job table in both stores, plus a separate quarantine
table, for embedding, session distillation and later jobs. A job is claimed with a
120-second lease. An expired lease counts as one attempt and then waits. Five
attempts is the cap. Waits after the first four failures are 30, 60, 120 and
240 seconds. The fifth failure quarantines. The worker polls every 2 seconds and embeds at most 8 memories per
tick. Succeeded rows are deleted after 24 hours. Quarantine rows stay until an
admin requeues or discards them.

`POST session/end` and the idle sweep enqueue a distill job and finish it before
returning, so the summary, handoff and typed notes are still stored before the
response. The poller finishes a job left behind by a crash. Search never enqueues
and never waits. A missing embedding model leaves embed jobs pending. IO and
inference failures retry. A bad vector, a batch-length mismatch, an unknown or
empty session, a session with nothing left to summarize, and a store conflict
quarantine immediately. A stale embedding publish, or a summary that is already
current, forgotten or superseded, is success. Explicit session end of a forgotten
summary still returns the validation error.

Queue counts and the quarantine list are admin-only. Public health is unchanged.
Exact fields and error classes are in the
[jobs contract](../contracts/continuity-v1.md#durable-jobs-v014).

### Stage 7 contract decisions (2026-11-17)

The owner confirmed declared contradictions, a similarity review job, and
explicit supersede on the existing `supersedes` field.

A declared link stores the contradicted id on the contradicting memory. Only
the contradicted memory is shaded, and only for a viewer who can read both
ends. The factor is `max(0.25, 1/(1+n))`, multiplied after the stage-3 fused
score and the stage-5 retention and feedback factors, before tiebreaks and
truncation. The memory stays in the result set. The lexical contradiction
signal is unchanged.

The similarity job runs on the stage-6 table under the key `similarity-scan`.
It uses stored vectors only. Cosine from 0.40 through 1.0 is reviewed; below
0.75 is a possible conflict and 0.75 and above is a near-duplicate. Each run
looks at the 60 coldest eligible memories, inserts at most 25 new open
reviews, skips captures, and schedules the next run an hour later. A review is
visible only when the viewer can read both memories. Dismiss changes the
review only. Marking a contradiction or a supersede from the review uses the
same writes as the direct actions.

Declare follows edit rights on the contradicted memory: its author or the
project owner, the personal author, or the global author or administrator.
Dismiss requires edit rights on both memories in the pair. Supersede requires
edit rights on both, because it writes the successor's `supersedes` field.
It does not call `correct`.

### Stage 8 contract decisions (2026-11-17)

The owner confirmed a memory link graph and one canvas. There is no code graph
in v0.1.4.

Typed links are `relates`, `causes`, `fixes`, and `depends_on`. Each is stored
on the source memory in both stores (`links` on the file record, migration
`0018_memory_links.sql` in PostgreSQL). Create and remove require read access
to both memories, the same scope and project, and edit rights on the source
row. `contradicts` and `supersedes` stay on their stage-7 fields. Recall walks
the new links one hop inside the existing link channel, in either direction,
with the same 512 caps. The relation does not change the channel weight.
`contradicts` and `supersedes` are not hops.

The graph shows every memory the viewer can already read, in one canvas, until
the viewer’s cap. The cap defaults to 120, and the viewer may set it from 120
through 500. The fewest incident edges are dropped first, then the oldest
memory, then the id. `truncated` reports that drop. An edge is included only
when both ends are visible. The console draws that response with Sigma.js and
graphology. Filters are scope, relation, project, and the cap. Import drops a
link that creation would reject (self-link, duplicate triple, or a target that
is missing, unreadable, or in another scope or project) and reports it on
`dropped_links`. Recall and the graph ignore a stored link that fails that rule.

### v0.1.5 client configuration (2026-11-18)

The selected JSON file is the only server URL and device credential for the CLI,
stdio MCP, and the Python hook, including hooks installed by `cairn setup`.
`CAIRN_CONFIG`, when non-empty after trimming, selects that file for the CLI, MCP,
and setup. An unmanaged hook `--config` path wins over `CAIRN_CONFIG`. An
installed hook is marked `--cairn-managed` and keeps its setup-time path as the
fallback; a non-empty `CAIRN_CONFIG` selects the runtime file instead. Otherwise
the file is `$XDG_CONFIG_HOME/cairn/config.json`, else `$HOME/.config/cairn/config.json`,
else `%APPDATA%/cairn/config.json`. `cairn setup <server-url>` still reads the
device token from stdin and writes the selected file.

`CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are not client or hook configuration. If
either name is present, including an empty or whitespace-only value, it is
ignored and one stderr line names the variable and not its value. A genuine
HTTP 401 names the selected file and tells the operator to run
`cairn setup <server-url>` with `CAIRN_CONFIG` set to that file. It does not
print the credential. Server process environment, outbox replay arguments, queue and retry
behavior, redaction, and account or device authorization are unchanged.

v0.1.5 adds no server-side LLM. Session distillation stays rule-based. Capture
and recall quality is the next section. Silent checkpoints, the identity block,
and the OpenCode plugin are the agent-continuity section after it. The v0.1.4
behavior above remains in force for the stages those sections do not change.

### v0.1.5 MCP results (2026-11-18)

Agents read a smaller MCP result. REST and the CLI keep the server JSON.

Search hits include `kind`. File ranking and PostgreSQL search, resume, and
unresolved all build the hit in `recall_result`, so both stores emit it.

`memory_save` and `memory_lesson_save` return `memory_id`, `kind`, `scope`, and
`status` on both MCP channels. A failed call stays an error. `memory_get` still
returns the whole record. `memory_export` stays lossless.

`memory_search`, `memory_smart_search`, `memory_recall`, `memory_context`, and
`memory_reflect` default to cards: `memory_id`, `kind`, `scope`, `title`,
`summary`, and `score`. A default card cuts `title` at 120 UTF-8 bytes and
`summary` at 240, on a character boundary, and ends a cut with `…`. Context
packs keep `query`, `results`, `omitted_count`, `omitted_memory_ids`, and
`recovery_hint`. `expanded: true` is the one wider view: useful recall fields,
without lifecycle, internal ids, or the duplicated `content` body, and without
an empty optional list. `memory_why_changed`, `memory_resume`, and
`memory_unresolved` stay full hits. Ranking and authorization are unchanged
except for the capture eligibility and relevance floor below.

### v0.1.5 capture and recall quality (2026-11-18)

Raw `capture` rows stay stored. Default agent search, smart search, recall,
context, reflect, and why-changed omit them until `includeCaptures` (or
`include_captures`) is true. The CLI flag is `--include-captures` on `search`
and `context`. Eligibility runs before ranking and the limit on both the file
store and PostgreSQL, so a hidden capture does not take a slot or move another
hit. Web list, graph, direct get, export, and project memory lists still return
captures. `memory_resume` and `memory_unresolved` keep listing session summaries;
they do not apply this filter or the floor.

Inferred keyword contradiction flags ignore captures on both sides. A declared
`contradicts` link or other stored provenance is left in place. Nothing is
deleted.

New captures get a short title (the tool name when the text is a tool envelope,
otherwise the first line, at most 80 characters). The observation text stays in
`content` and `details`. Older rows are not rewritten.

The hook drops a recognized read-only tool call before delivery and before the
outbox. Read, Grep, Glob, and the other named read tools are read-only. A shell
command is read-only only when the structured `command` or `cmd` string is one
simple program from a fixed list, or `git` with a read subcommand, and it has
no quotes, pipes, redirects, substitutions, or separators. A dash option is
read-only only when that command's allowlist names it. Anything ambiguous
or mutating is still captured. Nested `payload.tool_name` uses the same
envelope and excerpt cap as a top-level tool name. Skip markers, redaction, fingerprint idempotency,
replay, and auth are unchanged. Tool text is scrubbed and then clipped to 2048
UTF-8 bytes, including the truncation marker.

Session close still returns the session summary and stays idempotent. A handoff
is stored only when next steps or unresolved work were found. Typed notes and
their evidence stay. Next-step and unresolved extraction, and the summary text, skip an observation
whose role or source marks a tool payload (`PostToolUse` / `PreToolUse`, role
`tool` with a tool name followed by JSON, or role `tool` whose whole text is a
JSON object or array). Its event id stays on the summary. A JSON-looking line
inside prose is not the signal. Labeled prose that only uses the legacy tool
role still feeds notes and resume.

Agent search and context apply an absolute relevance floor after fusion and
before the limit. It is not a fraction of the highest score. A hit is kept when
the query has an exact entity, tag, symbol, file, command, test, or service
value; a technical token (digit or underscore) in the memory; a distinctive
token in the title; any distinctive token in a rule or decision; two distinct
distinctive tokens; or, for a note that is not a session summary or handoff,
one distinctive token of at least five characters. A repeated query word counts
once. A generated summary or handoff does not pass on one ordinary word such
as `config`. Stopwords fail. A linked neighbour needs its own evidence, so a
search can return nothing. Opt-in capture search uses the same floor. Resume,
unresolved, and why-changed do not. There is no model reranker and no server
LLM.

The local embedding channel still affects rank among hits that already pass
this floor. A hit with only vector similarity, even a high cosine against an
unrelated query, does not pass. Keeping those hits would need a new cosine
cutoff. That number is not in the approved contract, so agent search does not
invent one. Library search with the floor left off still returns the existing
semantic matches.

Tradeoff: a fact whose only overlap is one ordinary four-letter word in the
body is dropped, while a rule, a five-letter word on a note, or a long
identifier is kept. A session summary that only shares `config` with
`config reload` is dropped and remains available through resume. Link hops
no longer introduce a row that does not itself match the query.

Other MCP JSON is compact and drops nulls and empty strings. It keeps false,
zero, empty collections, status, and protocol flags. List results are objects
with a stable name, including an empty list, because MCP `structuredContent`
is an object.

Graph and similarity-review text is tab-delimited TOON (spec 4.4). Their
`structuredContent` stays JSON. Null `project_id` stays in the TOON table.
Tool names stay `memory_save` and `memory_get`. OpenCode already prefixes the
configured server name (`cairn_memory_save`), so the protocol names do not change.

### v0.1.5 agent continuity (2026-11-18)

A Stop hook no longer asks the agent to continue. After meaningful work it
posts `POST /cairn-memory/session/checkpoint` with the session id. The same
derive path as session close writes the summary and, when next steps or
unresolved work exist, the handoff. It does not set `ended_at`. A session that
is already closed, a summary that already covers the retained observations, or
a summary the person hid is left as stored. No retained observations return
`empty` and delete nothing. The response is `{"status":"stored"|"unchanged"|"empty"}`.
A checkpoint record is personal, tagged `continuity`, and keeps its project,
repository, branch, and worktree so the owner's brief can match them. Session
close still derives with the existing project rule unless it is only refreshing
a summary that already has that tag; it does not widen that handoff. File and
PostgreSQL both use this rule. A failed checkpoint leaves the hook's dirty flag
set so the next Stop retries. Clearing that flag matches the generation read
before the request, so a tool event during the request stays dirty. A read-only
tool, including a recognized read-only shell command, does not mark the session
dirty. The hook derives repository and branch from the checkout. `worktree` is a
stable hash of the canonical checkout root for every checkout, including the
main clone. An explicit `worktree` is kept. Older handoffs with no worktree
match only a request that also omits it.

The brief gains an `identity` array inside the existing fence. It holds at most
two active personal preferences or rules that were saved explicitly with the
tag `identity` and durable retention. Explicit saves already default to durable
retention, so the author uses `memory_save` or `cairn save` with scope
`personal`, kind `preference` or `rule`, and that tag. The block does not invent
account fields. Personal behaviour omits the same tag so the note is not
repeated. A handoff matches the requested worktree as well as repository,
branch, and project. A different worktree does not supply it.

The first structured `file_path`, `filePath`, or `path` on `Edit`, `Write`,
`MultiEdit`, or `apply_patch` asks `POST /cairn-memory/context` once per
repo-relative path per session. The query is that path, the limit is 3, and
the hook injects titles and summaries only. The same path, a read, or a shell
command does not search again. Raw file bodies are not sent.

Claude Code and Codex deliver post-compaction context through `SessionStart`
with `source` `compact` (`additionalContext`). Cairn's installed SessionStart
hook has no matcher, so compact uses the same brief request as startup.
`PreCompact` runs before compaction and is not installed. OpenCode 2.1.4's
documented delivery hook is `ctx.session.hook("compaction")`, with one following
`context` injection. A disposable `opencode serve` did not execute a local
plugin file, so the marked `AGENTS.md` block remains the fallback on that host.
The whole brief stays within 4096 UTF-8 bytes. Lower-priority sections are
shortened before identity and the handoff. Topic injection is fenced JSON
labelled as data.
`cairn setup --agent opencode` autoloads `plugins/cairn/index.js`. It does not
put a single file in the `plugins` array. Remove deletes that directory only
when the file starts with the Cairn marker, and it removes a config entry only
when the package path is the retired `plugin/cairn.js` asset with the same
marker. OpenCode 2.1.4 loaded the directory. Its plugin list stayed empty, and
session creation did not emit a session event.

### v0.1.6 explicit-save identity and review cleanup (2026-11-19)

The console Memories page loads records by default and no longer reads the
similarity review queue. A small on-demand control fetches open reviews and
shows only `near_duplicate` rows; historical `possible_conflict` rows remain
stored and readable through the API but are not presented there. Review
actions stay explicit and unchanged.

The background similarity scan no longer creates a review from cosine alone
below 0.75. It still publishes `near_duplicate` suggestions at 0.75 and above.
`possible_conflict` remains in the stored/API shape for historical rows and
compatibility, and neither band merges, hides, or replaces a memory. Cosine is
a suggestion, never a decision.

An explicit `memory_save` or `memory_lesson_save` reuses its existing record
only when the authenticated owner, scope, project, kind, lifecycle, context,
`supersedes`, and every meaning-bearing field match after redaction. A
temporary-retention expiry is normalized because it is generated per save; a
durable explicit expiry is compared. Evidence is excluded from that identity
and merged instead. Generated source event ids
and stored provenance are preserved. An identical retry with no new evidence
is a no-op, including `changed_at`. Different text, typed fields, owners,
kinds, scopes, context, lifecycle, or links create a separate record. Forgotten,
expired, superseded, and live-successor-hidden records are never reused. The
file store scans the caller's own live records; PostgreSQL serializes equal
saves with a transaction advisory lock keyed by owner and a SHA-256 identity
fingerprint, then compares the same identity in Rust. The evidence update
touches only `record`, `changed_by`, and `changed_at`; no migration or
re-embedding is needed. The MCP save receipt keeps its four fields. No
server-side LLM is added.

## Trust and limits

- Project membership is by project ID (ADR 0013), so raw captures, not only
  published decisions, are readable by anyone who joins. Redaction is the
  only barrier; it is best-effort.
- Any server user can read global notes and, through feedback, change their
  ranking. Edit and delete stay with author and administrator.
- Rule-based distillation is heuristic and can miss or mistype notes. Local
  embeddings add a model download and CPU cost to the server.
- Scope, record shape, rule-based distillation, hybrid retrieval, capture hygiene, the retention score, feedback, durable embedding and distillation jobs, declared contradiction handling, and the memory link graph are implemented. No code graph and no recall-quality benchmark are claimed. See the recorded [stage 1](../evidence.md#v014-stage-1--memory-scopes-and-record-shape), [stage 2](../evidence.md#v014-stage-2--capture-hygiene-and-session-distillation), [stage 3](../evidence.md#v014-stage-3--hybrid-recall), [stage 4](../evidence.md#v014-stage-4--session-brief-and-agent-instructions), [stage 5](../evidence.md#v014-stage-5--retention-and-feedback), [stage 6](../evidence.md#v014-stage-6--durable-jobs-and-quarantine), [stage 7](../evidence.md#v014-stage-7--contradiction-handling) and [stage 8](../evidence.md#v014-stage-8--memory-link-graph) checks.

## Implementation checklist

- [x] 0. Three scopes: scope parameter in MCP/CLI, per-repo personal scope and promote removed, captures in initialized repos go to project memory.
- [x] 1. Rule-based session-end distillation: session note, handoff, typed notes linked to captures.
- [x] 2. Hybrid recall: full-text, entity, links, local embeddings, RRF, type weights, pinned boost, scope labels.
- [x] 3. Bounded, data-fenced session-start brief from the hook.
- [x] 4. Retention score with reinforcement; rules/decisions exempt; no auto-delete.
- [x] 5. Capture hygiene: assistant text opt-in, skip marker file, scrub before truncation.
- [x] 6. Helpful/stale feedback affecting shared ranking.
- [x] 7. Memory schema (common + per-type fields) and migration (pre-release, no data compat); MCP, CLI and web forms show the fields.
- [x] 8. `cairn setup` installs agent instructions per agent (hook injection or instruction file).
- [x] 9. Start / first-work / end hooks with token bounds.
- [x] 10. Per-agent capability verification from official docs (hook events, context injection, continue request).

## Evidence

Source research: fact notes on akitaonrails/ai-memory and mempalace (decay with
access reinforcement, RRF ranking, sanitization hooks, opt-in assistant
capture). Related: [ADR 0013](./0013-shared-project-decisions-and-personal-memory-scopes.md),
[ADR 0010](./0010-correction-keeps-linked-history.md).

Stage 1 implementation evidence: `crates/core/src/memories.rs`, `capture.rs`, `recall.rs`, `sessions.rs`; `crates/storage/src/lib.rs`; both server routers; `crates/client/src/tools.rs`; and `apps/web/app/memories/page.tsx`. Scoped HTTP tests run identically on both adapters in `crates/server/tests/project_model.rs`; typed round trips and title-weight regressions are in the core sibling tests and PostgreSQL integration tests. The development base migration now stores the flat record in `record` JSONB and derives a title-weighted lexical index. Reset older databases/file stores before running this build: old private `global` values must never be interpreted as server-wide global notes.

Stage 2 implementation evidence: `scripts/cairn_hook.py` and its authentication,
offline and outbox smokes; `crates/core/src/distillation.rs` and sibling tests;
session-end paths in both stores; shared HTTP scenarios in
`crates/server/tests/project_model.rs`; and the end-to-end full-flow, hook and
MCP smokes. [Recorded checks](../evidence.md#v014-stage-2--capture-hygiene-and-session-distillation)
cover idempotence, evidence, project/private visibility, late captures,
correction, forgetting and distinct derived-ID namespaces.

Stage 3 implementation evidence: `crates/core/src/hybrid.rs` and sibling tests;
the file and PostgreSQL embedding adapters; `crates/server/src/embeddings.rs`;
and `crates/server/tests/semantic.rs`, which exercises real local MiniLM
backfill and all five retrieval routes on both stores. The production image
uses Debian trixie slim for ONNX's C/C++ runtime requirements. The CLI keeps
static musl Linux releases; Linux server releases use GNU targets. See
[release requirements](../release.md#014-upgrade-notes) and the recorded
[checks and resource measurements](../evidence.md#v014-stage-3--hybrid-recall).

Stage 4 implementation evidence: `crates/core/src/brief.rs` and sibling tests;
both server `/session/brief` handlers and shared HTTP visibility scenarios;
`scripts/cairn_hook.py`, hook lifecycle tests and `scripts/cairn_setup.py` with
JSONC/preservation tests; `crates/client/src/agents.rs`, setup integration tests,
and the shared instruction text used by MCP/hooks/OpenCode. See the
[stage 4 check record](../evidence.md#v014-stage-4--session-brief-and-agent-instructions).

Stage 5 implementation evidence: `crates/core/src/retention.rs` and sibling
tests; ranking in `crates/core/src/recall.rs` and
`crates/storage/src/embeddings.rs`; migration `0015_retention_feedback.sql`;
feedback on both HTTP adapters, the MCP/CLI tool table, and the web memory
sheets. See the
[stage 5 check record](../evidence.md#v014-stage-5--retention-and-feedback).

Stage 6 implementation evidence: `crates/core/src/jobs.rs` and sibling tests;
file-store claims in that module; `crates/storage/src/jobs.rs`, migration
`0016_background_jobs.sql` and `crates/storage/tests/jobs.rs`; the worker in
`crates/server/src/embeddings.rs`; admin routes on both HTTP adapters; and the
console page `apps/web/app/jobs/page.tsx`. See the
[stage 6 check record](../evidence.md#v014-stage-6--durable-jobs-and-quarantine).

Stage 7 implementation evidence: `crates/core/src/contradictions.rs` and sibling
tests; ranking in `crates/core/src/recall.rs` and
`crates/storage/src/embeddings.rs`; migration `0017_contradictions.sql`;
review storage in both adapters; MCP/CLI tools; and the Memories page review
list. See the
[stage 7 check record](../evidence.md#v014-stage-7--contradiction-handling).

Stage 8 implementation evidence: `crates/core/src/links.rs` and sibling tests;
the one-hop walk in `crates/core/src/hybrid.rs`; migration
`0018_memory_links.sql` and `crates/storage/src/contradictions.rs`; both HTTP
adapters; MCP/CLI `memory_link`, `memory_unlink`, and `memory_graph`; and
`apps/web/app/graph/page.tsx`. See the
[stage 8 check record](../evidence.md#v014-stage-8--memory-link-graph).
