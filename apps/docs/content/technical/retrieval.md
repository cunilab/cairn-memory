---
id: CM-REF-004
title: "Retrieval and ranking"
status: accepted
description: "How Cairn combines evidence, filters visibility, ranks records and fits them into context."
---

# Retrieval and ranking

## Hybrid recall

Search applies visibility and context filters, then combines lexical, entity, vector and link evidence. Vector search uses the local 384-dimensional MiniLM model when configured; lexical, entity and link search continue without it. Reciprocal-rank fusion combines candidate channels. The relevance gate accepts an entity match, cosine at least 0.43, or at least half the query words with cosine at least 0.35 when an embedding is available. Without an embedding, coverage alone can pass. Results are ranked and packed under a limit and token budget; `more` reports omitted results. A relevant paraphrase can miss the gate.

Project scope requires membership. Global memories match every context; a personal handoff also follows the repository, branch and worktree context. Candidate notes rank lower and stay out of L1. A helpful vote can confirm a coached agent note, and a clean correction saves at normal rank. System notes can also be confirmed through use. Proof counts add a bounded ranking boost. Stale and superseded memories rank lower and carry a label. Forgotten memories are excluded. Aliases may be stored but are not currently used in vector search. While that gate is off, new alias vectors are not generated; existing alias rows and vectors are kept. See [the benchmark](../../../../bench/README.md) for the gate's limits.

## Automatic recall evidence

File topics match the exact file, its module or symbol names from a bounded sample of tool input. Results rank rules and gotchas first, followed by decisions, then facts and procedures. Within a group, helpful votes take precedence over recency. Each pushed memory includes a short reason for showing it.

A command warning can show up to two gotchas or procedures once per normalized command per session. It matches an exact command entity after collapsing spaces, stopping at the first pipe, `&&`, semicolon or output redirect, and taking the first three words. The warning preserves the host’s permission flow.

Failed-command recall uses the first error-looking line in the last 2 KiB of redacted output. A match requires the exact command entity and a phrase of at least three adjacent search lexemes. Gotchas rank ahead of procedures. If the host confirms failure and supplies only status, recall matches by command entity. An unknown status, weak match or timeout is silent. The lookup uses PostgreSQL text and entity indexes and leaves the error tail out of capture and memory storage. See the [host hook matrix](hooks/index.md#command-recall-by-host) for delivery timing.

Prompt recall uses the hybrid channels for personal preferences and rules, and for rules, decisions, gotchas and procedures in the current project. It skips slash commands, prompts shorter than four words, and acknowledgements without a task. It requires at least 0.60 absolute relevance before ranking adjustments. The search gate can accept 0.50 lexical coverage or a cosine-only match, so 0.60 asks for stronger evidence before injecting context. A prompt that uses an explicit cue such as “with Sol” can match the name in a preference's title or wording at the existing 0.70 entity-evidence floor. The cue still needs a prompt of at least four words. Each prompt can add up to two memory cards and one skill card in 300 UTF-8 bytes, with a reason on each card. The client waits up to one second for recall. The recall endpoint uses the redacted prompt for lookup only. Prompt capture follows its existing rules.

[Recall layers and packing limits](../users/memory/layers.md#automatic-recall) · [MCP result contracts](../users/reference/mcp.md#results)
