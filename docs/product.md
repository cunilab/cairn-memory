---
id: CM-PRODUCT-001
title: "Product — Cairn Memory"
status: draft
---

# Product — Cairn Memory

> Cairn Memory is an AI-managed, person-owned memory layer that automatically captures, organizes, synchronizes, and retrieves useful context for people and coding agents.

## Product Statement

**For** developers and coding agents who lose decisions, discoveries, project context, and workflow knowledge between sessions,  
**Cairn Memory** is an AI-managed personal memory layer  
**that** automatically captures useful evidence, derives durable memories, and assembles the right context across sessions, projects, agents, and devices.  
**When trusted and explicitly enabled,** the same memory can be promoted into a project or team cairn without giving up provenance or ownership.

Unlike a generic chat-history store, shared notes application, or manually maintained wiki, Cairn Memory lets AI manage memory relevance while preserving provenance, project scope, identity, visibility, lifecycle state, and the reasons behind technical decisions. It is a source that both humans and agents can query, inspect, and correct.

## Product thesis

**Cairn remembers the work so people and agents do not have to.** The default experience should require no manual save, tagging, filing, or summarization. Cairn's AI manages capture, classification, compression, retention, and retrieval. The person remains the owner and controls privacy, correction, deletion, export, and sharing.

This creates three explicit boundaries:

- **AI controls relevance** — what is useful, how it is compressed, how long it is retained, and when it is retrieved.
- **Policy controls trust** — who may see a memory, whether it may be promoted, and how revocation works.
- **The person controls ownership** — correction, deletion, export, privacy defaults, and access to the person's memory.

## Product Model

| Concept | Meaning |
| --- | --- |
| **Actor** | A person, agent, service, or device that creates, transforms, reads, or shares memory |
| **Memory event** | An append-only observation or state transition with provenance and causal metadata |
| **Memory** | A derived or curated unit of knowledge assembled from one or more events |
| **Space** | A visibility and authorization boundary such as personal, project, team, or organization |
| **Cairn** | A useful shared context assembled from multiple memory events and made available to authorized actors |
| **Trail** | The source and change history that explains why a memory exists and how it evolved |
| **Memory autopilot** | The automated capture, classification, compression, retention, retrieval, and correction loop that manages memory without requiring manual filing |
| **Evidence** | A source observation, file, symbol, command, test, commit, or other record that supports or invalidates a memory |

## Problem

**Current situation:** People and coding agents work across short-lived sessions, multiple machines, branches, and tools. They repeatedly rediscover repository conventions, previous fixes, architectural decisions, project status, and working agreements.

**Problem:** Important context is scattered across transcripts, files, prompts, chat systems, issue trackers, and human memory. People are forced to decide what to save and where to put it, while agents often start with too little context or too much irrelevant history. Personal memory does not automatically follow a user to another device, and useful project context does not safely flow to collaborators or agents. Automatic capture also creates privacy, consent, conflict, durability, and trust concerns.

**Impact:** People and agents repeat work, make inconsistent decisions, lose continuity between sessions and collaborators, miss important handoffs, and consume more context and user time than necessary.

## Users

| User | Need | Current alternative |
| --- | --- | --- |
| Individual developer | Automatic memory that follows them across sessions, devices, and agents without manual filing | Re-explain context, search transcripts, or maintain manual notes |
| AI coding agent | Fast, scoped, relevant context with provenance and evidence | No memory, prompt-injected notes, or agent-specific local files |
| Project team | A trusted project cairn built from verified personal and branch context | Chat history, wikis, issue trackers, and meetings |
| Workspace administrator | Identity, permissions, retention, audit, and recovery when shared spaces are enabled | Manual access management and opaque hosted memory services |
| Integration author | Stable REST, MCP, sync, and hook interfaces | Custom adapter for each agent and memory product |

## Goals

- [ ] **AI-managed personal memory** — Cairn captures, classifies, compresses, retains, and retrieves useful memory without requiring the person to decide what to save or where to file it.
- [ ] **Useful continuity** — an agent can recover relevant prior decisions, discoveries, evidence, and workflows without replaying an entire session.
- [ ] **Cross-device continuity** — a person's permitted memory is available on their connected devices without manual export/import.
- [ ] **Branch-aware project context** — branch and worktree hypotheses remain scoped until they are merged, confirmed, or explicitly promoted.
- [ ] **Evidence-backed promotion** — useful personal or branch memory can become project memory with source evidence, freshness, and an explainable promotion path.
- [ ] **Cross-person collaboration** — as a later layer, authorized collaborators and agents can discover shared project memory without exposing private memory.
- [ ] **Automatic memory lifecycle** — capture, classification, deduplication, summarization, retention, correction suggestions, and synchronization happen automatically within explicit policy boundaries.
- [ ] **Trustworthy retrieval** — results expose enough provenance and scope for a user or agent to judge whether they apply.
- [ ] **Server-authoritative durable memory** — when server mode is enabled, the server is the canonical source for synchronized personal, project, and shared memory; local clients provide capture buffering and outage tolerance.
- [ ] **Useful edge operation** — each device can capture during a short server outage and replay safely without becoming a competing collaborative source of truth.
- [ ] **Safe collaboration** — private memory remains private by default and shared memory has clear identity, permissions, provenance, and revocation behavior.
- [ ] **Compatibility** — existing agent hooks, REST clients, MCP clients, and export data can migrate incrementally.
- [ ] **Operational durability** — accepted observations survive transient failures and restarts, with explicit recovery behavior.
- [ ] **Inspectable product** — users can understand, search, review, correct, export, forget, and audit memory through a lightweight viewer and APIs without manually maintaining a knowledge base.

## Scope

### In

- Session and observation capture for coding-agent workflows.
- AI-managed personal memory autopilot with automatic retention and evidence trails.
- Durable memories, summaries, lessons, provenance, and lifecycle management.
- Identity for people, agents, devices, workspaces, and service actors.
- Personal, project, team, and organization memory spaces with explicit sharing rules, introduced as separate maturity layers.
- Repository, branch, worktree, file, symbol, command, test, and commit context.
- Cross-device synchronization with offline capture, idempotency, cursors, and conflict handling.
- Cross-person collaboration through shared memory, handoffs, decisions, and project context.
- Keyword, semantic, and later graph-assisted retrieval.
- REST and MCP interfaces for agent integrations.
- Local persistence, export/import, privacy filtering, audit trails, retention, and recovery.
- A lightweight Next.js viewer for search, sessions, memory trails, exceptions, sync health, and governance controls.

### Out

- Replacing the coding-agent runtimes themselves.
- Becoming a general-purpose project-management, chat, or social-network product.
- Requiring users to manually organize, tag, or save every useful memory.
- Unrestricted automatic sharing of every captured event.
- Treating the sync service as the only place where memory can be queried.
- Mandatory cloud hosting, billing, or subscription tiers before the local and self-hosted model is validated.
- Hiding memory decisions, permissions, or conflict resolution behind an uninspectable service.

## Product Principles

- **AI-managed, human-owned** — the AI manages relevance, compression, retention, and retrieval; the person owns the memory and controls privacy, correction, deletion, export, and sharing.
- **Private by default** — automatic capture is not automatic sharing; sharing must follow explicit space and policy boundaries.
- **Personal before collaborative** — a person should receive value from Cairn before project and team sharing are enabled.
- **Server authoritative, edge tolerant** — the server owns canonical memory; clients buffer capture and provide limited outage tolerance.
- **Evidence over narrative** — preserve source observations, provenance, timestamps, and scope so summaries can be questioned.
- **Identity before trust** — every write, transformation, share, read, and revoke operation has an actor and an authorization context.
- **Append before merge** — preserve source events and explain derived merges instead of overwriting history invisibly.
- **Compatibility before optimization** — preserve agent-facing contracts before changing internals or ranking algorithms.
- **Progressive disclosure** — return compact context first and allow deeper inspection when needed.
- **Explicit lifecycle** — memory may be captured, compressed, consolidated, corrected, superseded, forgotten, or deleted.
- **Provider agnostic** — LLMs and embedding providers enrich memory but should not be required for basic capture and keyword recall.
- **Safe failure** — capture, search, and background enrichment fail independently where possible; a provider outage must not destroy stored memory.

## UI role

The UI is a viewer and control surface, not a manual knowledge-base editor. It should answer:

1. What does Cairn remember?
2. Why did Cairn remember or retrieve it?
3. What is uncertain, stale, conflicting, or sensitive?
4. What can the person correct, forget, export, or share?

The default interface should be exception-driven: show useful context automatically and ask for attention only when confidence, privacy, conflict, sync, or sharing requires a human decision.

## Assumptions

| Assumption | Evidence | Status |
| --- | --- | --- |
| People will allow AI-managed automatic capture when ownership and controls are clear | Capture is the foundation of the AgentMemory product, but the person must not be forced to curate every memory | Unvalidated |
| Memory should be private by default and shared through spaces and policy | Cross-person memory without explicit boundaries creates unacceptable leakage risk | Product decision; needs validation |
| A central sync plane is acceptable for personal cross-device continuity | It simplifies recovery and delivery compared with peer-to-peer federation | Unvalidated |
| A server-authoritative database is the right default for personal and project memory | Durable retrieval, device continuity, and later collaboration need one permissioned source of truth | Product decision; needs architecture validation |
| A local spool/cache is sufficient for outage tolerance | The edge should not become a second collaborative database | Design hypothesis |
| Append-only events plus derived memories are easier to reconcile than last-write-wins records | Collaboration needs provenance, conflict visibility, and replay | Design hypothesis |
| People need to inspect automatic memory decisions without managing every memory | Trust depends on explanations, evidence, correction, and exception handling | Unvalidated |

## Success

| Signal | Current | Target |
| --- | --- | --- |
| Automatic memory usefulness | Not measured | A person can use Cairn without manually saving or filing memories, and the system retains useful context across representative sessions |
| Session resumption | Not measured | A new agent session reaches its first informed action faster than a session without Cairn, measured on a versioned task corpus |
| Retrieval relevance | Not measured | Retrieved context is judged relevant by the user or agent on a versioned golden corpus, with stale and incorrect results tracked separately |
| Repeated investigation avoided | Not measured | Cairn measurably reduces repeated searches, explanations, and failed approaches across follow-up sessions |
| Server-authoritative recall | Not measured | A capture accepted by the server is available to every authorized client without manual export |
| Outage tolerance | Not measured | Client capture queues safely during a short outage and replays exactly once after reconnect |
| Authorized shared memory | Not measured | A permitted collaborator can recall shared context; an unauthorized actor cannot |
| Offline capture | Not measured | Offline events synchronize once and remain available after reconnect |
| Automatic sharing safety | Not measured | Sensitive or private material is not published to a shared space without policy approval |
| Secret leakage | Not measured | Zero known secrets are persisted, sent to a provider, or published through automatic memory handling |
| Search quality after migration | Not measured for the rewrite | No unexplained regression against a versioned golden corpus |
| User ability to inspect and correct memory | No rewrite console yet | Viewer supports search, detail, provenance, exceptions, correction, forget, export, and governance flows |
| Data portability | Upstream export exists; rewrite compatibility not measured | A versioned export can be imported into a clean rewrite instance |
