---
id: CM-SPEC-003
title: "Specification — Graph-Assisted Retrieval"
status: draft
---

# Specification — Graph-Assisted Retrieval

| Field | Value |
| --- | --- |
| Status | Draft |
| Owner | Cairn Memory maintainers |
| Target | Layer 6: advanced intelligence and operations; deterministic code context may begin in Layers 2–4 |
| Related | [Architecture](../architecture.md), [Memory context and promotion](./memory-context-and-promotion.md), [ADR 0003](../adr/0003-server-authoritative-postgres.md) |

## Summary

Embedding similarity alone is not enough for coding-agent memory. Agents need to preserve file identity, symbol relationships, imports, calls, tests, configuration links, decisions, sessions, branches, commits, and the path by which an answer was discovered. Cairn Memory should use graph-assisted retrieval combined with lexical and vector retrieval, but only after simpler personal and branch-aware retrieval has demonstrated where graph context materially improves outcomes.

The [Graphify](https://github.com/Graphify-Labs/graphify) approach is a useful reference: deterministic AST parsing can create explainable code relationships locally, while extracted and inferred edges remain distinguishable. Cairn should adapt that idea to a server-side, branch-aware, permission-aware memory graph rather than copy a static graph file workflow.

Graph relationships are a retrieval and evidence capability, not a reason to make the first usable Cairn depend on a graph database or unrestricted traversal. Deterministic file, symbol, test, and commit links may enter the project-memory layer early; bounded graph-assisted ranking belongs to the advanced-intelligence layer of the roadmap.

## Graph layers

### 1. Code graph

Deterministically extract, where possible:

- repository, branch, worktree, commit;
- files and directories;
- modules, classes, functions, methods, types, and symbols;
- imports, calls, inheritance, implementations, tests, configuration references, and generated relationships.

Store source range, parser version, commit SHA, and edge type. Code parsing should not require an LLM.

### 2. Work graph

Capture relationships from agent activity:

- session read file;
- session edited file;
- command affected service;
- observation mentioned symbol;
- decision changed configuration;
- commit contains file;
- pull request changed symbol;
- agent asked about entity;
- test verified or invalidated a memory.

### 3. Memory graph

Represent:

- memory supports or contradicts memory;
- memory derived from observations;
- memory applies to project, branch, worktree, or organization;
- lesson reinforced by later evidence;
- decision supersedes prior decision;
- user preference differs from project convention.

Every edge carries provenance, confidence, scope, actor, and created-at metadata.

## Edge labels

Use explicit evidence labels:

```text
EXTRACTED  directly present in source or tool event
RESOLVED   deterministically resolved from known identifiers
INFERRED   derived by a model or heuristic
AMBIGUOUS  candidate relation requiring review
```

Inference must never silently become an extracted fact.

## Retrieval pipeline

```text
query
  -> identity and authorization filter
  -> query normalization / optional expansion
  -> lexical candidates (PostgreSQL FTS)
  -> semantic candidates (pgvector)
  -> graph seed entities
  -> bounded graph expansion
  -> candidate fusion
  -> optional reranker
  -> freshness/confidence/scope/diversity ranking
  -> context pack with evidence paths
```

Graph expansion should be bounded by hop count, node count, bytes, scope, and time. It should enrich lexical/vector candidates, not scan an unrestricted graph on every request.

## Embedding policy

- The server must have an embedding pipeline for production semantic retrieval.
- LLM generation is optional and cannot block capture or lexical retrieval.
- The default deployment should support a local server-side embedding model so memory content need not leave the deployment.
- The initial Rust candidate is `fastembed-rs` with `BAAI/bge-small-en-v1.5` for a lightweight English baseline. Evaluate `BAAI/bge-m3` for multilingual or longer-context deployments and `jinaai/jina-embeddings-v2-base-code` for code-heavy retrieval; do not mix model families in one index without explicit model metadata and re-embedding.
- Store model ID, version, dimension, normalization, source fingerprint, and embedding status.
- A model change creates a versioned re-embedding job; old and new vectors may coexist during migration.
- Query and document embeddings must use the same model version for a retrieval stream.
- For code, embeddings are one signal; symbol/path/branch/commit and graph relationships remain first-class.

## PostgreSQL retrieval design

- PostgreSQL FTS stores searchable text and metadata for lexical recall.
- pgvector stores dense embeddings and supports exact search first, then HNSW after measurement.
- Relational graph tables store nodes, edges, source references, scope, and parser/model versions.
- Use tenant/space filters before returning candidates; approximate vector filtering needs explicit index and query testing.
- Fuse streams with reciprocal-rank fusion or another measured ranker.
- Keep an exact-search evaluation path so approximate retrieval can be compared for recall.

## Context continuity behavior

When an agent has read a file or symbol, the system should retain a compact evidence record containing:

- file path and repository;
- branch/worktree and commit;
- symbol or line range when available;
- tool/session/agent identity;
- neighboring graph entities;
- why it was read;
- related decisions, tests, and errors;
- whether the connection was extracted, resolved, inferred, or ambiguous.

After compaction or a new session, Cairn can rebuild a context pack from these records instead of relying only on a prose summary. This directly addresses the failure mode where compression preserves the conclusion but loses the file and relationship context.

## Acceptance criteria

- [ ] A query about a function can retrieve its file, callers, tests, related decisions, and recent edits with explainable paths.
- [ ] A branch-local code relationship does not leak into another branch without matching repository/commit context.
- [ ] Graph edges show whether they were extracted, resolved, inferred, or ambiguous.
- [ ] Authorization filtering occurs before graph expansion and before vector results are returned.
- [ ] Semantic retrieval can run with a local server-side embedding model and no LLM generation key.
- [ ] The system can compare exact and approximate vector search recall.
- [ ] A context pack preserves file and relationship evidence after session compression.

## Reference material

- [Graphify](https://github.com/Graphify-Labs/graphify) — deterministic AST/code-graph and explainable extracted versus inferred edges.
- [CM-RESEARCH-001](../research/memory-confidence-and-correction.md) — source survey of confidence scales and correction links. The Graphify rubric recorded there is not this specification's scoring rule.
- [pgvector](https://github.com/pgvector/pgvector) — PostgreSQL vector storage, exact/approximate search, filtering, and hybrid-search patterns.
- [fastembed-rs](https://github.com/Anush008/fastembed-rs) — local Rust embeddings and reranking with ONNX/Candle-backed model options.
