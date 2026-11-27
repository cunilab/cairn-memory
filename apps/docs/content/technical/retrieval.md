---
id: CM-REF-004
title: "Retrieval and ranking"
status: accepted
description: "How Cairn combines evidence, filters visibility, ranks records and fits them into context."
---

# Retrieval and ranking

```mermaid
flowchart LR
  accTitle: Explicit retrieval pipeline
  accDescr: Search filters records by visibility, combines evidence from the available channels, gates and ranks results, and packs compact cards into the token budget.
  Scope[Visibility] --> Evidence[Fusion] --> Rank[Ranking] --> Pack[Packing]
```

Cairn merges candidate lists with weighted reciprocal-rank fusion. The [relevance gate](../glossary.md#relevance-gate) drops weak candidates before trust and status affect ranking. Matching records become compact cards within the token budget, which defaults to 1,200 tokens. Each card can include `why`, the channels that matched and a file, symbol, command or error label. The reply includes a `more` count for additional matches. `memory_get` can ask for one field (`field`) or a short supersedes chain (`timeline`) for a single id the caller can already read.

A person can turn a search trace on in Settings. For 7 days Cairn then stores that person's redacted query, result ids, ranks and channel names, and marks a result opened or voted. The trace holds no memory text, does not change trust or usage, and is visible only to that person.

## Hybrid recall

Search applies visibility filters before querying candidate channels:

- **Selectors**: Explicit search accepts `recorded_from`, `recorded_to`, `branch` and `version`. A bare date is that UTC day. Nothing in the query text is read as a date. Hooks do not send these selectors.
- **Lexical channel**: PostgreSQL full-text search using English stemming. Explicit search also ORs the parts of an unprotected snake_case, camelCase or path stem into the partial-match leg. Quoted text, versions and error lines stay whole. Hooks keep the original query. After the relevance gate, equal scores prefer an exact symbol, path or error, then a memory listed by more channels. Each memory is returned once.
- **Vector channel**: 384-dimensional cosine similarity via pgvector and local `all-MiniLM-L6-v2`.
- **Entity channel**: Exact matches on symbol names, file paths, and command signatures.
- **Link channel**: Traversing `supersedes` and relationship edges.
- **Relevance gate**: Requires:
  1. An exact entity hit, OR
  2. Cosine similarity ≥ 0.43, OR
  3. Lexical coverage ≥ 50% with cosine similarity ≥ 0.35.

## Automatic recall evidence

| Trigger | Search criteria | Priority & limits |
| --- | --- | --- |
| **File topics** | Matches edited or read file paths, module names, or symbols | Rules & Gotchas > Decisions > Facts & Procedures. Up to 4 cards of 300 bytes. |
| **Command warnings** | Exact normalized command entity (first 3 words before `\|`, `&&`, `;`) | Up to 2 gotchas or procedures once per command per session. |
| **Failed command** | Error line in last 2 KiB output + command entity | Gotchas precede procedures. Matches error signature with ≥ 3 lexemes. |
| **Prompt recall** | Hybrid search for prompts ≥ 4 words (relevance ≥ 0.60) | Up to 2 memory cards and 1 skill card within 300 bytes total. |

See [Recall layers and packing limits](../users/memory/layers.md#automatic-recall) and [MCP result contracts](../users/reference/mcp.md#results).
