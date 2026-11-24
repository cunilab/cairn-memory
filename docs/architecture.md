---
id: CM-ARCH-001
title: "Architecture — Cairn Memory"
status: draft
---

# Architecture — Cairn Memory

Cairn is a self-hosted memory service. A Rust server owns durable state in PostgreSQL with pgvector. Agents use the hook, CLI and MCP through device-authenticated HTTP; people use the web console through web sessions.

## Components

| Component | Responsibility |
| --- | --- |
| `cairn-core` | Types and pure logic for memory, capture, processing, retrieval, projects, accounts and fixes. |
| `cairn-store` | PostgreSQL queries, authorization, transactions, migrations and jobs. |
| `cairn-server` | HTTP routes, authentication and background workers. |
| `cairn-client` | CLI, MCP, hook, setup and updates; communicates over HTTP. |
| `apps/web` | Console for accounts, devices, memories, projects, activity, graph and Fixes. |

```mermaid
flowchart LR
    Hook[Agent hook] -->|device token| API[REST API]
    Client[CLI / MCP] -->|device token| API
    Web[Web console] -->|web session| API
    API --> Store[cairn-store]
    Worker[Background worker] --> Store
    Store --> PG[(PostgreSQL + pgvector)]
    Repo[.cairn/project.json] --> Hook
    Repo --> Client
```

Dependencies run from core to store to server; the client uses HTTP rather than opening the store. The worker handles distill, embed, link and consolidate jobs, plus retention and Fixes sweeps. MiniLM runs locally when configured; without it retrieval can use the other channels.

## Data and visibility

Accounts, web sessions, devices and audit events hold identity and authorization data. Projects and members track the shared project ID. Memories, links, entities, usage and aliases describe durable knowledge; sessions and observations hold capture evidence. Jobs track background work; Fixes and lookup logs support diagnosis. Server settings hold global agent instructions, with project overrides on projects.

A memory is personal, project or global. Reads apply ownership and membership checks before returning records. Forgotten memories are excluded from normal recall. Stale and superseded records remain searchable with lower rank and a label; age alone does not hide a memory. Raw observations are retained separately and are not recalled as memory.

## Main flows

```mermaid
flowchart TD
    Event[Hook event] --> Redact[Redact and clip]
    Redact --> Observe[Store observation under session]
    Observe --> Distill[Checkpoint or end: distill]
    Distill --> Notes[Candidate notes, digest and handoff]
    Save[Agent save] --> Validate[Redact and validate]
    Validate --> Memory[Memory and links]
    Notes --> Memory
    Memory --> Jobs[Embed, link and consolidate jobs]
    Query[Search] --> Visibility[Apply visibility]
    Visibility --> Channels[Text, entity, link and vector channels]
    Channels --> Gate[Relevance gate and ranking]
    Gate --> Results[Compact cards or brief]
```

Capture stays personal when project attachment fails; recognized projects can receive distilled notes. The hook uses a bounded outbox during outages. Explicit saves are validated and deduplicated; corrections name the old memory in `supersedes`. Forget hides the retained record from normal recall. Feedback can mark a memory stale. A project file identifies membership, so protect it as shared access to the project's memories.

## Decisions and why

- Project owners rename in the web console. Joining uses the project ID even if the file has an old name; `cairn init` updates that name, and session-start notices remind teammates to update and commit the file.

- **Server-authoritative PostgreSQL:** one store and one authorization boundary keep shared state consistent; pgvector supports local semantic retrieval.
- **Self-hosted accounts:** the first account is the administrator; password and device credentials are stored as hashes. Humans manage devices in the web console.
- **Linked corrections:** a replacement links to the previous record, preserving provenance and making both searchable.
- **Scopes and hybrid recall:** personal, project and global scopes set visibility; text, entities, links and local embeddings improve retrieval without broadening access.
- **Small agent surface:** five MCP tools, Rust hooks and no server LLM keep operation predictable and avoid an external model dependency.

See the [release process](release.md) for release and update decisions, and the [user guide](user-guide.md) for operation.
