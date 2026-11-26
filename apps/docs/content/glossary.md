---
id: CM-DOC-GLOSSARY
title: "Glossary"
status: accepted
description: "Look up Cairn memory kinds, recall layers, trust labels, and integration terms."
---

# Glossary

## Records and memory kinds

### Memory

A saved record with a kind, title, summary, owner, and scope. It can also carry a reason, evidence, detail, and fields specific to its kind. [Kinds and records](users/memory/kinds.md)

### Decision

A choice and the reason behind it, such as using disposable schemas for tests.

### Rule

A standing constraint or instruction, including any known exceptions.

### Gotcha

A verified failure pattern with a symptom, cause, or fix.

### Procedure

A repeatable workflow with steps and guidance on when to use it.

### Fact

Verified information about a project, environment, or system.

### Preference

A person's working convention, such as how they want an agent to handle a choice.

### Note

General context that does not require fields specific to another kind. A note can be saved without a reason.

### Handoff

A system-generated continuity record from a session. It can carry unfinished work, next steps, and the latest execution result into a later checkout session. [Session handoffs](technical/processing.md#session-handoffs-and-distillation)

### Evidence

File paths, URLs, or other references that help someone verify a record. The reason explains why the record is worth keeping.

### Detail

Optional long-form content on a memory. Search returns compact results, and a caller can request the full detail by ID.

### Alias

An alternate question used to express how someone might look for a memory. Aliases are stored and embedded. Alias vector search is disabled in the shipped retrieval configuration described in [Benchmarks](technical/benchmarks.mdx).

### Link

A typed edge between memories. Cairn uses relations such as `relates`, `causes`, `fixes`, `depends_on`, `supersedes`, and `contradicts` to connect records and preserve history.

## Visibility and access

### Scope

The visibility boundary on a memory. Scope answers who can access a record. [Projects and scopes](users/memory/scopes.md)

### Personal scope

Visible to the owning account. A personal memory tagged to a Hermes chat also has a [chat boundary](#chat-tag).

### Project scope

Visible to members of the associated project. Archived project records remain available for read-only inspection in the console.

### Global scope

Visible to authenticated accounts on the Cairn server. The skill library is also shared server-wide.

### Project

A memory namespace shared by members on a Cairn server. An account with the project ID in `.cairn/project.json` can join. [Project membership](users/memory/scopes.md#init-and-membership)

### Checkout

A local copy of a repository. Separate branches or worktrees can carry different handoff context while sharing project memory.

### Archive

A reversible action that hides a project from agent recall or an archived skill from skill search. Archived records remain stored. [Archive and restore](users/memory/scopes.md#archive-and-restore)

### Account

A person's identity on the Cairn server. The first account is the sole administrator, and other people have their own accounts.

### Device token

A revocable bearer credential issued from the web console. The CLI, hooks, and integration authenticate as the token's account and use that account's project memberships. [Device tokens](technical/security.md#device-tokens)

### Web session

A cookie-backed sign-in used by people in the console. It authenticates account and device management actions.

### Chat tag

A tag that limits a gateway's chat-scoped personal record to the originating thread and the gateway account's web session. Explicit project records follow project membership. [Hermes chat recall](users/agents/hermes-gateway.md#hermes-chat-recall)

## Trust and record status

### Trust level

A confidence label derived from the record's rank, status, and feedback. [Trust labels](users/memory/trust.md#trust-labels)

### New

A record with candidate rank, such as a distilled observation or an unverified group-chat save. Candidates stay out of the L1 brief.

### Candidate

A stored record awaiting verification. Candidate is a rank, and New is the trust label for a candidate without a stronger warning.

### Normal

The usual trust label for a verified record without enough helpful feedback or repeated saves to qualify as Proven.

### Proven

Trust earned through at least two helpful votes or at least three saves of the same knowledge. It can give a memory priority in L1.

### Doubted

A trust warning shown after a wrong vote or when a record is stale.

### Stale

A record status that lowers its rank and labels it in search. Feedback marks a record stale after one wrong vote from the owner or wrong votes from two people.

### Active

The record status for a memory that has not been marked stale, superseded, or forgotten.

### Superseded

A record replaced by a newer one through a `supersedes` link. Cairn retains the old record and its history.

### Contradiction

A `contradicts` link records disagreement with another memory without replacing it.

### Forgotten

A record hidden from normal recall by its owner. Age alone does not hide a memory.

### Feedback

A helpful or wrong vote. Memory votes affect confidence, and skill votes apply to a specific skill version.

### Muting

The automatic suppression of a repeatedly shown memory that goes unused. An explicit search hit, a fetch, or a helpful vote can unmute it. [Delivery rules](users/memory/layers.md#delivery-rules-and-budgeting)

## Recall and context

### Recall

Delivery of relevant stored knowledge to an agent. Recall can happen automatically at session start or during work, or on request through search. [Recall layers](users/memory/layers.md)

### Session brief

Opening context supplied to a supported agent. Pins lead the brief, followed by applicable handoff and essential memories.

### L0 Identity

Personal and current-project pins, oldest first, at the start of the brief.

### L1 Essential

Checkout handoff, personal preferences, and eligible project rules, decisions, and gotchas. Proven records have priority.

### L2 Contextual

Cards delivered when relevant to a file, command, failure, or prompt. A session ledger limits repeat delivery and counts cards against the session budget.

### L3 Search

Explicit search, such as an agent calling `memory_search` or a person running `cairn search`.

### Pin

An owned personal or project memory selected to lead eligible briefs in L0. [Pin management](users/memory/pins.md#pin-management)

### Card

A compact summary that a brief or tool reply can include. The memory ID lets an agent fetch the full record.

### Hybrid retrieval

Search that combines lexical, vector, entity, and link evidence. The relevance gate filters weak matches before the reply is packed into a context budget. [Hybrid recall](technical/retrieval.md#hybrid-recall)

### Lexical match

A match on words or word stems, scored by the full-text search channel.

### Entity

An extracted file, module, symbol, command, test, service, or term that search can match to a record.

### Embedding

A numeric vector used to compare text by meaning. Cairn can calculate embeddings with local MiniLM and store them using PostgreSQL with pgvector.

### Relevance gate

A filter that requires strong enough entity, lexical, or vector evidence before returning a search match.

### Token budget

A limit on how much search output is sent back to an agent. Briefs and contextual cards have separate byte limits.

## Capture and processing

### Capture

Collection of selected events from an agent session, such as prompts, edits, and tool results. Cairn applies redaction before storing observations. [Hook protocol](technical/hooks/index.md)

### Hook

An integration callback that runs when the agent starts, handles a tool call, receives a prompt, reaches a checkpoint, or ends a session. Host capabilities determine which events are available.

### Observation

A redacted event stored for background processing. Observations have a retention period and are excluded from memory search.

### Distill

Process observations into candidate notes, handoffs, or project digests at a checkpoint or session end. [Session handoffs and distillation](technical/processing.md#session-handoffs-and-distillation)

### Checkpoint

A lifecycle boundary, such as a stop event or pre-compaction, at which Cairn can queue distillation before the session ends.

### Digest

A summary of recent activity in an attached project, generated during background processing.

### Job

A persisted background task such as distilling observations, embedding a memory, linking records, or identifying duplicates. [Durable jobs](technical/processing.md#durable-jobs)

### Outbox

A bounded queue on a device that retries delivery of captured events after transient failures. PostgreSQL on the server remains the authoritative store. [Offline outbox](technical/hooks/index.md#outbox)

## Skills and integrations

### Skill

A reusable Markdown procedure in a shared, versioned library. Saving the same skill name creates a new immutable revision. [Skills](users/memory/skills.md)

### MCP

Model Context Protocol. Supported agents call Cairn tools through the local `cairn mcp` process, which talks to the Cairn server. [MCP tools](users/reference/mcp.md#tools)

### Provider

The Hermes integration that exposes Cairn as the host's native memory provider, with prompt prefetch and memory and skill tools. [Hermes Agent](users/agents/hermes.md)

### Gateway

A dedicated Hermes profile for messaging platforms. It tracks per-thread sessions and can share records through an explicitly chosen project. [Hermes chat gateway](users/agents/hermes-gateway.md)

### Web console

The browser interface for inspecting memories, projects, sessions, skills, and devices. The administrator also manages accounts and system Fixes here. [Web console](users/web-console.md)

### Fixes

Reviewable actions the console surfaces for an administrator to address certain memory or system issues.
