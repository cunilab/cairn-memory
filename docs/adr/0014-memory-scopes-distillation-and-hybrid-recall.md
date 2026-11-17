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
  [lifecycle contract](../contracts/lifecycle-v1.md#forgetting-shared-knowledge).

### Stage 3 contract decisions (2026-11-17)

The owner approved `fastembed` 7.7.1 with minimal CPU ONNX features and the
SHA-verified FP32 MiniLM artifact pinned in `models/minilm.json` (Apache-2.0).
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
[export contract](../contracts/export-v1.md#import-behavior) owns that rule.

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

## Trust and limits

- Project membership is by project ID (ADR 0013), so raw captures, not only
  published decisions, are readable by anyone who joins. Redaction is the
  only barrier; it is best-effort.
- Any server user can read global notes and, through feedback, change their
  ranking. Edit and delete stay with author and administrator.
- Rule-based distillation is heuristic and can miss or mistype notes. Local
  embeddings add a model download and CPU cost to the server.
- Scope, record shape, rule-based distillation, hybrid retrieval, capture hygiene, the retention score, feedback, and durable embedding and distillation jobs are implemented. No recall-quality benchmark is claimed. See the recorded [stage 1](../evidence.md#v014-stage-1--memory-scopes-and-record-shape), [stage 2](../evidence.md#v014-stage-2--capture-hygiene-and-session-distillation), [stage 3](../evidence.md#v014-stage-3--hybrid-recall), [stage 4](../evidence.md#v014-stage-4--session-brief-and-agent-instructions) and [stage 5](../evidence.md#v014-stage-5--retention-and-feedback) checks.

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
