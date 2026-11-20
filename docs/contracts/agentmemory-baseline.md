---
id: CM-CONTRACT-001
title: "AgentMemory Compatibility Baseline"
status: draft
---

# AgentMemory Compatibility Baseline

| Field | Value |
| --- | --- |
| Scope | The upstream AgentMemory behavior Cairn must preserve, adapt, or deliberately defer |
| Snapshot date | 2026-10-25 |
| Upstream | [rohitg00/agentmemory](https://github.com/rohitg00/agentmemory), `main` snapshot |
| Observed package | `@agentmemory/agentmemory` `0.9.30` |
| Runtime baseline | Node.js 20+, iii-engine `0.22.1`, local keyless operation |

## Purpose

This document is the Phase 0 contract inventory for the Cairn rewrite. It separates user-visible behavior from the upstream implementation. Cairn may replace TypeScript, iii-engine, local storage, and provider internals, but it must not silently break the capture, recall, integration, lifecycle, and portability behaviors that existing AgentMemory users rely on.

The inventory is intentionally explicit about verification gaps. A contract marked **must preserve** is a compatibility target, not a claim that Cairn has already implemented it.

## Compatibility categories

| Category | Meaning |
| --- | --- |
| **Must preserve** | Keep the public name, intent, and observable success/error behavior unless a migration plan and compatibility test approve a change |
| **Can simplify** | Preserve the user outcome while changing the internal mechanism or optional surface |
| **Later** | Valuable upstream behavior that belongs after the personal-memory vertical slice |
| **Not part of Cairn v1** | Explicitly outside the first product; may still be supported by an adapter or import path |
| **Verification gap** | The behavior is identified, but the exact payload/schema or edge case still needs a source-level fixture before implementation |

## Contract inventory

| Surface | Observed upstream contract | Cairn classification | Target layer | Verification |
| --- | --- | --- | --- | --- |
| Automatic capture hooks | `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, and `SessionEnd` are present in the plugin hook manifest | Must preserve the capture intent; hook commands may change | Layer 1 | Source manifest reviewed; adapter-specific extra hooks remain a follow-up |
| Observation capture | `POST /agentmemory/observe` accepts session, content, role, source, importance, and metadata fields | Must preserve the accepted observation concept and idempotent behavior | Layer 1 | Request field examples recorded below; exact source schema still needs a fixture |
| Explicit memory save | `POST /agentmemory/remember` and the `memory_save` MCP tool | Must preserve for explicit correction, import, and compatibility; normal use should not require it | Layer 1 | REST example and core tool listed below |
| Session lifecycle | `POST /agentmemory/session/start`, `/session/end`, `/session/active`, and `/sessions` | Must preserve the session identity and recovery behavior | Layer 1 | Endpoint names observed; exact response schemas need source fixtures |
| Recall | `/search`, `/smart-search`, `/context`, `/recall`, plus MCP recall/search tools | Must preserve core recall semantics; authorization and scope filtering are Cairn extensions | Layer 1–2 | Endpoint names observed; ranking parity requires a golden corpus |
| Forget/delete | `/forget`, `memory_governance_delete`, and lifecycle operations | Must preserve user-controlled forget/correction behavior; deletion guarantees need an ADR and test | Layer 1 | Exact deletion/tombstone semantics are a Phase 0 open decision |
| Health and readiness | `/agentmemory/health` and `/agentmemory/livez` | Must preserve operational probes | Layer 1 | Verified from upstream install/runbook documentation |
| MCP core surface | `memory_save`, `memory_recall`, `memory_consolidate`, `memory_smart_search`, `memory_sessions`, `memory_diagnose`, `memory_lesson_save`, `memory_reflect` | Must preserve names and intent for the core set | Layer 1 | Core set documented by upstream runbook; Cairn Layer 1 mapping is in [CM-CONTRACT-002](mcp-layer1.md) |
| MCP full surface | Default `--tools all` exposes 54 tools; `--tools core` exposes 8 | Can simplify internally; preserve the core set first and inventory the rest before migration | Layer 1–6 | Count and core set observed; full registry diff remains a pre-Layer-1 task |
| Export/import | `export`, `import`, and `import-jsonl` support portability and Claude Code transcript import | Must preserve a versioned import/export path; raw upstream schema must be mapped, not copied blindly | Layer 1 | Cairn v1 is defined in [CM-CONTRACT-003](../archive/contracts/export-v1.md); transcript import remains later |
| CLI lifecycle | Start, `init`, `connect`, `status`, `doctor`, `demo`, `upgrade`, `stop`, `mcp`, `capture`, `compression`, `graph-index`, and `import-jsonl` | Must preserve the useful diagnostic and lifecycle outcomes; package onboarding prompts can simplify | Layer 1 | Help surface reviewed from upstream source |
| Agent wiring | `connect <agent>` merges MCP/configuration for supported hosts and preserves existing entries | Must preserve non-destructive integration behavior | Layer 1 | Supported-agent matrix needs a Cairn adapter test |
| Viewer | Local viewer defaults to port `3113` and shows live memory state | Can simplify to a useful observer/control surface | Layer 1 | Port and viewer behavior observed |
| Runtime ports | REST/MCP `3111`, streams `3112`, viewer `3113`, iii engine `49134`; `--instance` derives isolated ports | Can simplify the engine topology; preserve configurable service discovery and health | Layer 1 | Defaults observed in upstream docs and config |
| Provider fallback | Keyless mode captures and recalls with BM25; LLM compression and semantic embeddings are opt-in | Must preserve the no-provider fallback; provider breadth can simplify | Layer 1 | Keyless behavior observed; Cairn policy is recorded in `docs/policy.md` |
| Graph retrieval | `memory_smart_search` can combine BM25/vector/structural graph results; graph extraction is configurable | Later, with deterministic code evidence allowed earlier | Layer 2–6 | Graphify relationship model is a reference, not a first-layer dependency |
| Team mode | `TEAM_MODE`, `TEAM_ID`, and `USER_ID` can scope upstream memories | Later collaboration compatibility; not the default personal-memory model | Layer 5 | Behavior needs a dedicated shared-space fixture |
| Optional exports/bridges | Claude memory bridge, Obsidian export, snapshots, Redis state backend, and provider-specific knobs | Later or adapter-specific | Layer 6 / not v1 | Preserve data portability without making each optional integration a core dependency |

## REST baseline

The following is the initial route inventory extracted from the upstream README and installation runbook. The names and intent are compatibility targets. Request and response schemas must be pinned in contract tests before the Rust implementation claims parity.

| Route | Required baseline behavior | Initial request fields |
| --- | --- | --- |
| `GET /agentmemory/health` | Return service health and provider/index state | None |
| `GET /agentmemory/livez` | Return readiness and discovered viewer information | None |
| `POST /agentmemory/remember` | Store an explicit durable memory and return a created result | `content`, optional `concepts` |
| `POST /agentmemory/observe` | Accept an observation for a session and make it searchable | `sessionId`, `content`, `role`, `source`, optional `importance`, `metadata` |
| `POST /agentmemory/session/start` | Start or resume a session identity | `sessionId`, optional `agent`, `project`, metadata |
| `POST /agentmemory/session/end` | Finalize a session and trigger available lifecycle work | `sessionId` |
| `GET /agentmemory/session/active` | Report active session state | Optional project/agent filters |
| `GET /agentmemory/sessions` | List session summaries or session records | Optional paging and project filters |
| `POST /agentmemory/search` | Return lexical or configured hybrid matches | Query, limit, scope filters |
| `POST /agentmemory/smart-search` | Return fused search and structural matches | Query, limit, scope filters |
| `POST /agentmemory/context` | Assemble bounded context for an agent | Query/session/project context, token budget |
| `POST /agentmemory/recall` | Return a recall-oriented context/result payload | Query and scope filters |
| `POST /agentmemory/forget` | Apply a user-authorized forget/delete transition | Target identity and authorization context; exact body is a verification gap |
| `POST /agentmemory/export` / import equivalent | Produce or restore a portable versioned representation | File/format/options; exact schema is a verification gap |

## Hook and observation baseline

The Phase 0 schema is deliberately transport-neutral. A hook adapter may send directly to REST, through a local spool, or through an MCP/iii bridge, but the resulting event must carry enough information for personal continuity and later evidence-backed promotion.

| Field | Requirement | Notes |
| --- | --- | --- |
| `eventId` | Stable idempotency key when the adapter can provide one | Required for replay-safe delivery in Cairn |
| `sessionId` | Stable session identity | Upstream route examples use `sessionId` |
| `content` | Bounded observation text or normalized tool event | Must be redacted before provider calls or sharing |
| `role` | Source role such as user, agent, tool, or system | Preserve enough source identity for provenance |
| `source` | Capture source such as hook, import, REST, or MCP | Used to explain why a memory exists |
| `metadata` | Structured context including tool, project, branch, files, or timestamps | Must be size-bounded and schema-versioned |
| `importance` | Optional upstream hint | Cairn treats it as evidence, not authorization |
| `repository`, `branch`, `worktree`, `commit` | Optional coding context | Required when available for Layer 2 continuity |

## CLI baseline

| Command or option | Classification | Cairn behavior |
| --- | --- | --- |
| Default start | Must preserve | Start or attach to the configured local/server runtime |
| `init` | Can simplify | Create a safe configuration template without requiring a provider key |
| `connect <agent>` | Must preserve outcome | Merge integration configuration without replacing unrelated user settings |
| `status` / `doctor` | Must preserve | Show health, provider/index mode, ports, and actionable failures |
| `demo` | Can simplify | Keep a small deterministic smoke fixture for local validation |
| `stop` | Must preserve | Stop only the runtime owned by the selected instance/configuration |
| `mcp` | Must preserve outcome | Provide a standalone MCP path for MCP-only clients |
| `capture` / `compression` / `graph-index` | Later but inspectable | Preserve diagnostic concepts; exact command surface may change in Rust |
| `import-jsonl` | Must preserve portability | Import existing agent transcripts without exposing secrets or making import the only capture path |
| `--data-dir` / `AGENTMEMORY_DATA_DIR` | Must preserve outcome | Keep state outside the repository by default and support explicit relocation |
| `--instance` / `--port` | Can simplify | Keep isolated local instances and predictable service discovery |
| `--tools all|core` | Can simplify | Preserve a lean core-tool mode even if the full tool count changes |

## Environment variable catalog

The names below are the Phase 0 compatibility catalog. Cairn should preserve names where migration friction would be high, document aliases when names change, and never copy secrets into repository fixtures or logs.

| Area | Variables in the upstream baseline |
| --- | --- |
| Capture | `AGENTMEMORY_CAPTURE_ALLOW`, `AGENTMEMORY_CAPTURE_DENY`, `AGENTMEMORY_CAPTURE_OUTPUT_MAX`, `AGENTMEMORY_PRE_COMPACT_BUDGET`, `AGENTMEMORY_CAPTURE_MAX_ATTEMPTS`, `AGENTMEMORY_CAPTURE_RETRY_INTERVAL_MS`, `AGENTMEMORY_CAPTURE_DEDUP_HOURS`, `AGENTMEMORY_CAPTURE_EVENTS_MAX`, `AGENTMEMORY_CAPTURE_INBOX_MAX`, `AGENTMEMORY_CAPTURE_DEAD_MAX`, `AGENTMEMORY_CAPTURE_SPOOL`, `AGENTMEMORY_CAPTURE_SPOOL_DIR`, `AGENTMEMORY_CAPTURE_SPOOL_MAX_BYTES`, `AGENTMEMORY_CAPTURE_SPOOL_MAX_AGE_HOURS` |
| Providers | `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_API_KEY_FOR_LLM`, `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL`, `ANTHROPIC_BASE_URL`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `GEMINI_MODEL`, `OPENROUTER_API_KEY`, `OPENROUTER_MODEL`, `MINIMAX_API_KEY`, `MINIMAX_MODEL`, `MAX_TOKENS`, `AGENTMEMORY_LLM_TIMEOUT_MS`, `FALLBACK_PROVIDERS`, `AGENTMEMORY_ALLOW_AGENT_SDK` |
| Embeddings and search | `EMBEDDING_PROVIDER`, `VOYAGE_API_KEY`, `COHERE_API_KEY`, `OPENAI_EMBEDDING_API_KEY`, `OPENAI_EMBEDDING_MODEL`, `OPENAI_EMBEDDING_DIMENSIONS`, `BM25_WEIGHT`, `VECTOR_WEIGHT`, `AGENTMEMORY_GRAPH_WEIGHT`, `TOKEN_BUDGET`, `MAX_OBS_PER_SESSION` |
| Privacy and auth | `AGENTMEMORY_SECRET`, `VIEWER_ALLOWED_ORIGINS`, `AGENTMEMORY_IMPORT_ROOT`, `TEAM_MODE`, `TEAM_ID`, `USER_ID`, `AGENT_ID`, `AGENTMEMORY_AGENT_SCOPE` |
| Lifecycle | `AGENTMEMORY_AUTO_COMPRESS`, `AGENTMEMORY_INJECT_CONTEXT`, `CONSOLIDATION_ENABLED`, `CONSOLIDATION_DECAY_DAYS`, `AGENTMEMORY_REFLECT`, `GRAPH_EXTRACTION_ENABLED`, `GRAPH_EXTRACTION_BATCH_SIZE`, `AGENTMEMORY_AUDIT_RETENTION_MONTHS`, `AGENTMEMORY_SESSION_SWEEP_ENABLED`, `AGENTMEMORY_SESSION_SWEEP_STALE_HOURS`, `AGENTMEMORY_FINALIZE_IDLE_MS` |
| Runtime and storage | `AGENTMEMORY_URL`, `AGENTMEMORY_VIEWER_URL`, `AGENTMEMORY_DATA_DIR`, `AGENTMEMORY_III_CONFIG`, `AGENTMEMORY_STATE_BACKEND`, `AGENTMEMORY_REDIS_URL`, `AGENTMEMORY_STATE_SAVE_INTERVAL_MS`, `AGENTMEMORY_USE_DOCKER`, `AGENTMEMORY_TOOLS`, `AGENTMEMORY_SLOTS`, `AGENTMEMORY_DEBUG`, `AGENTMEMORY_FORCE_PROXY`, `AGENTMEMORY_READY_TIMEOUT_MS`, `AGENTMEMORY_ENGINE_READY_TIMEOUT_MS`, `AGENTMEMORY_III_VERSION`, `STANDALONE_MCP`, `STANDALONE_PERSIST_PATH` |
| Ports | `III_REST_PORT`, `III_STREAM_PORT`, `III_STREAMS_PORT`, `III_VIEWER_PORT`, `III_ENGINE_PORT`, `III_ENGINE_URL` |

## Lifecycle baseline

```text
hook or import
  -> bounded observation
  -> secret/privacy filtering
  -> idempotent capture
  -> synthetic compression and BM25 indexing by default
  -> optional provider compression, embeddings, graph extraction, and consolidation
  -> session summary / lesson / durable memory
  -> recall or context injection when explicitly enabled
  -> correction, forget, export, or promotion
```

Important upstream behavior to preserve:

- A provider key is not required for capture or BM25 recall.
- LLM-written observation compression is separately gated by `AGENTMEMORY_AUTO_COMPRESS`.
- Context injection is separately gated by `AGENTMEMORY_INJECT_CONTEXT`.
- Embeddings are an opt-in retrieval stream, not a requirement for keyless operation.
- Captured observations must survive provider failure and be available for later retry.
- Export/import must be versioned and bounded so a backup can be restored without replaying provider calls.

## Verification gaps before Layer 1

1. Pin the upstream commit used for the final contract fixture rather than relying only on `main`.
2. Extract the full MCP tool registry and compare names, arguments, result shapes, and error behavior.
3. Extract exact REST request/response schemas, especially `forget`, session endpoints, export, and import.
4. Extract the hook payload schemas for each supported adapter and normalize them into the event fixture format.
5. Record exact export fields, schema versions, tombstone behavior, and import limits.
6. Add contract tests for keyless capture, provider failure, duplicate replay, restart persistence, and scope filtering.

## Sources

- [Upstream README](https://github.com/rohitg00/agentmemory/blob/main/README.md)
- [Upstream package manifest](https://github.com/rohitg00/agentmemory/blob/main/package.json)
- [Upstream environment template](https://github.com/rohitg00/agentmemory/blob/main/.env.example)
- [Upstream plugin hooks](https://github.com/rohitg00/agentmemory/blob/main/plugin/hooks.json)
- [Upstream install and verification runbook](https://github.com/rohitg00/agentmemory/blob/main/INSTALL_FOR_AGENTS.md)
