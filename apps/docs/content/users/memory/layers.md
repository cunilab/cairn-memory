---
id: CM-DOC-USERS-MEMORY-LAYERS
title: "Recall layers"
status: accepted
description: "See which context Cairn delivers automatically and what agents can request."
---

# Recall layers

L0 and L1 form the opening [session brief](../../glossary.md#session-brief). L2 delivers context as the agent works. L3 answers explicit search requests.

## Automatic recall

| Layer | Content | Budget | Delivery trigger |
| --- | --- | --- | --- |
| **[L0 Identity](../../glossary.md#l0-identity)** | Personal pins (max 3) and current project pins (max 4), split 50/50 with spillover, oldest first | 900 UTF-8 bytes (~225 tokens) | Session start brief |
| **[L1 Essential](../../glossary.md#l1-essential)** | Checkout handoff, personal preferences and rules, project rules, decisions, and gotchas | 900 UTF-8 bytes (~225 tokens) | Session start, after L0 |
| **[L2 Contextual](../../glossary.md#l2-contextual)** | File topics, command warnings, failed-command recall, and prompt cards | 300 UTF-8 bytes per card (max 8 cards/session) | Tool execution, command errors, or prompt match |
| **[L3 Search](../../glossary.md#l3-search)** | Hybrid retrieval results via `memory_search` | Configured token budget (1,200 default) | Explicit agent tool call |

### Delivery rules and budgeting

- **Session brief cap**: Fits within 1,800 bytes (900 B L0 + 900 B L1) when the client requests `extended_brief: true`. Standard clients receive up to 1,200 bytes total (up to 600 B L0, with L1 using the remainder).
- **L0 pin split and overflow**: Personal and project pins each get half of L0, with unused room spilling over to the other scope. When not all pins fit, L0 reports `"N more pinned not shown"`.
- **L1 ranking**: The checkout handoff comes first, with its task, edited files, last run, counts and first next step (its card may use 520 of the 900 bytes; a long task is cut before the rest). The other cards follow, proven memories before normal ones, sorted by combined fetches and helpful votes, with recent updates breaking ties.
- **Session ledger**: Up to eight L2 cards are shared across file topics, command warnings, error recall, and prompts per session. Brief (L0/L1) cards are recorded in the session ledger to prevent repeats and count usage without consuming the 8-card L2 budget.
- **Skill prompt recall**: Prompts outside Hermes gateway chats can inject one matching skill card (requires ≥ 0.60 lexeme overlap).
- **[Muting](../../glossary.md#muting)**: Three consecutive pushes across separate sessions with zero uses mute a memory from automatic recall. Muting is immediately lifted by an explicit search hit, `memory_get`, or helpful vote.

[Pin a memory](pins.md) · [Use search](recall.md) · [Technical ranking and gates](../../technical/retrieval.md)
