---
id: CM-DOC-USERS-MEMORY-LAYERS
title: "Recall layers"
status: accepted
description: "See which context Cairn delivers automatically and what agents can request."
---

# Recall layers

## Automatic recall

| Layer | Content | Budget | When |
| --- | --- | --- | --- |
| L0 Identity | Personal pins and pins in the current project, oldest pin first | 600 UTF-8 bytes (~150 tokens) | Session start |
| L1 Essential | The checkout handoff, personal preferences and current project rules, decisions and gotchas | 1,200 UTF-8 bytes (~300 tokens) | Session start, after L0 |
| L2 Automatic recall | File topics, command warnings, failed-command and prompt recall | 300 UTF-8 bytes per reply and eight cards per session shared with L0 and L1 | When a tool, command or prompt matches |
| L3 Search | Hybrid results from `memory_search` | Requested token budget, 1,200 by default | On explicit lookup |

The session brief fits within 1,800 bytes including the labels and data fence when the hook or MCP initialize requests it with `extended_brief: true`. Without that flag, the brief keeps the same L0 and fits L1 into the space remaining under the 1,200-byte total cap. L1 shows proven memories before normal ones, then sorts by combined fetches and helpful votes, with most recently updated first on ties. It observes per-person mutes and keeps the existing project branch and checkout filters. Brief and automatic recall cards show age since the original save.

Prompt recall outside Hermes gateway chats can include one skill per session. The active skill's title and description must match at least 0.60 of the prompt's English search lexemes. The card shows the title, description and match reason. It shares the existing prompt reply and session budgets. Use `skill_get` to read the body.

L1 and L2 require normal or proven trust. L0 reflects pins selected by the owner. The server shares an eight-card budget across briefs, file topics, command warnings, failed-command recall and prompt recall for each person and session ID. A memory can use only one slot in that session ledger. Each skill card uses one slot. The idle sweep removes ledger rows after seven days and keeps aggregate use counts.

Cairn credits a pushed card once per session when the agent gets the memory or votes on it. For failed-command cards with an identifiable error, Cairn can also credit use at session close when the error did not recur. The idle sweep handles hosts without session-end hooks. A matching later failure revokes inferred credit while the ledger is retained. Cairn uses a hash of the normalized, redacted error line to recognize a repeat. A status-only failure can retrieve a card, with use credited through gets or votes. The memory sheet displays the current person's push and use counts.

After three pushes in separate sessions with zero uses, that person's automatic recall mutes the memory unless it is pinned. The person can still find or get it, and its trust stays the same. A returned explicit search result, a successful get, or a helpful vote restores automatic eligibility. Session attribution comes from either the host's MCP session metadata or its post-tool hook.

[Pin a memory](pins.md) · [Use search](recall.md) · [Technical ranking and gates](../../technical/retrieval.md)
