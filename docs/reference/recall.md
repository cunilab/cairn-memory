---
id: CM-REF-004
title: "Recall and background processing"
status: draft
---

# Recall and background processing

## Hybrid recall

Search applies visibility and context filters, then combines lexical, entity, vector and link evidence. Vector search uses the local 384-dimensional MiniLM model when configured; lexical, entity and link search continue without it. Reciprocal-rank fusion combines candidate channels. The relevance gate accepts an entity match, cosine at least 0.43, or at least half the query words with cosine at least 0.35 when an embedding is available. Without an embedding, coverage alone can pass. Results are ranked and packed under a limit and token budget; `more` reports omitted results. A relevant paraphrase can miss the gate.

Project scope requires membership. Global memories match every context; a personal handoff also follows the repository, branch and worktree context. Candidate notes rank lower and stay out of L1. A helpful vote can confirm a coached agent note, and a clean correction saves at normal rank. System notes can also be confirmed through use. Proof counts add a bounded ranking boost. Stale and superseded memories rank lower and carry a label. Forgotten memories are excluded. Aliases may be stored but are not currently used in vector search. While that gate is off, new alias vectors are not generated; existing alias rows and vectors are kept. See [the benchmark](../../bench/README.md) for the gate's limits.

## Automatic recall

| Layer | Content | Budget | When |
| --- | --- | --- | --- |
| L0 Identity | Personal pins and pins in the current project, oldest pin first | 600 UTF-8 bytes (~150 tokens) | Session start |
| L1 Essential | The checkout handoff, personal preferences and current project rules, decisions and gotchas | 1,200 UTF-8 bytes (~300 tokens) | Session start, after L0 |
| L2 Automatic recall | File topics, command warnings, failed-command and prompt recall | 300 UTF-8 bytes per reply and eight cards per session shared with L0 and L1 | When a tool, command or prompt matches |
| L3 Search | Hybrid results from `memory_search` | Requested token budget, 1,200 by default | On explicit lookup |

The session brief fits within 1,800 bytes including the labels and data fence when the hook or MCP initialize requests it with `extended_brief: true`. Without that flag, the brief keeps the same L0 and fits L1 into the space remaining under the 1,200-byte total cap. L1 shows proven memories before normal ones, then sorts by combined fetches and helpful votes, with most recently updated first on ties. It observes per-person mutes and keeps the existing project branch and checkout filters. Brief and automatic recall cards show age since the original save.

Owners can pin or unpin memories in the web memory sheet. The web route is `POST /cairn-memory/memories/{id}/pin` with a Boolean `pinned` field. A person can pin up to five personal memories and a project holds up to ten pins across its members. Pins bypass muting. Archived project, forgotten and superseded memories stay out of L0. A superseded pin retains its slot until unpinned so restoring a correction preserves the quota. Forgetting a memory frees its slot.

File topics match the exact file, its module or symbol names from a bounded sample of tool input. Results rank rules and gotchas first, followed by decisions, then facts and procedures. Within a group, helpful votes take precedence over recency. Each pushed memory includes a short reason for showing it.

A command warning can show up to two gotchas or procedures once per normalized command per session. It matches an exact command entity after collapsing spaces, stopping at the first pipe, `&&`, semicolon or output redirect, and taking the first three words. The warning preserves the host’s permission flow.

Failed-command recall uses the first error-looking line in the last 2 KiB of redacted output. A match requires the exact command entity and a phrase of at least three adjacent search lexemes. Gotchas rank ahead of procedures. If the host confirms failure and supplies only status, recall matches by command entity. An unknown status, weak match or timeout is silent. The lookup uses PostgreSQL text and entity indexes and leaves the error tail out of capture and memory storage. See the [host hook matrix](hook.md#command-recall-by-host) for delivery timing.

Prompt recall uses the hybrid channels for personal preferences and rules, and for rules, decisions, gotchas and procedures in the current project. It skips slash commands, prompts shorter than four words, and acknowledgements without a task. It requires at least 0.60 absolute relevance before ranking adjustments. The search gate can accept 0.50 lexical coverage or a cosine-only match, so 0.60 asks for stronger evidence before injecting context. A prompt that uses an explicit cue such as “with Sol” can match the name in a preference's title or wording at the existing 0.70 entity-evidence floor. The cue still needs a prompt of at least four words. Each prompt can add up to two cards in 300 UTF-8 bytes, with a reason on each card. The client waits up to one second for recall. The recall endpoint uses the redacted prompt for lookup only. Prompt capture follows its existing rules.

L1 and L2 require normal or proven trust. L0 reflects pins selected by the owner. The server shares an eight-memory budget across briefs, file topics, command warnings, failed-command recall and prompt recall for each person and session ID. A memory can use only one slot in that session ledger. The idle sweep removes ledger rows after seven days and keeps aggregate use counts.

Cairn credits a pushed card once per session when the agent gets the memory or votes on it. For failed-command cards with an identifiable error, Cairn can also credit use at session close when the error did not recur. The idle sweep handles hosts without session-end hooks. A matching later failure revokes inferred credit while the ledger is retained. Cairn uses a hash of the normalized, redacted error line to recognize a repeat. A status-only failure can retrieve a card, with use credited through gets or votes. The memory sheet displays the current person's push and use counts.

After three pushes in separate sessions with zero uses, that person's automatic recall mutes the memory unless it is pinned. The person can still find or get it, and its trust stays the same. A returned explicit search result, a successful get, or a helpful vote restores automatic eligibility. Session attribution comes from either the host's MCP session metadata or its post-tool hook.

## Hermes chat recall

Cairn saves chat memories as personal records owned by the gateway's Cairn account.
The same chat, that owner's verified DM with the bot, and the owner's web console can
read those records. Other chats and standalone agent sessions leave chat-tagged
memories out of their results. A group or an unmapped DM can read personal memories
tagged to its own chat. L0 pins, L1 essentials, L2 prompts and commands, and explicit
search use the same privacy rule. Project recall in chat requires an explicit existing
project link and the owner's project membership. Group and unmapped DM saves start
as new candidates, including repeat saves and memories derived from captured turns.

Each chat thread has its own Cairn session and eight-card automatic recall ledger.
A chat without threads uses one session. At 30 minutes of idle time Cairn closes
the session and queues distillation. The next message starts a new ledger. A
gateway restart starts a new session on the next message. Cairn's idle sweep closes
any earlier session with retained captures. See [Hermes gateway setup](hook.md#hermes-telegram-and-discord-gateway).

## Session handoffs and distillation

Checkpoint and session-end processing turns typed captures into notes, a digest and, when appropriate, a handoff. A session that mixes projects, or project and personal work, distills each attachment on its own; a mixed or indeterminate cohort stays personal. The digest's test line is the last execution of that command. Explicit `next:` or `todo:` lines supply next steps. Short chats and automated or subagent sessions do not overwrite a useful handoff. Raw observations are not returned by memory search.

## Durable jobs

The worker runs distill, embed, link and consolidate jobs. Jobs use leases, retry with backoff and quarantine on repeated failure. The console shows quarantined jobs; a sweep and a server-version change can retry them. Finished jobs and raw observations have retention limits, while memories do not expire by age.

Consolidation can link related records or supersede a near duplicate. Differences in numbers, versions or file names veto an automatic merge. Explicit corrections use `supersedes`; declared contradictions are kept as links. Search cards give compact results; `memory_get` returns full records and links.
