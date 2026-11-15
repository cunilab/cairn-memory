# Cairn Memory

> An AI-managed, person-owned memory layer that helps coding agents and people resume work without repeating context.

**Status:** Layer 5 implemented · `apps/web` is the first Next.js console · **Target stack:** Rust · Next.js · shadcn/ui

This repository is the planning and implementation home for **Cairn Memory**. It is independently rebuilt and inspired by [AgentMemory](https://github.com/rohitg00/agentmemory) for capture, compression, retrieval, lifecycle, and agent integrations, and by [Graphify](https://github.com/Graphify-Labs/graphify) for deterministic, explainable code relationships. Cairn first makes one person's memory automatic and useful, then adds branch-aware project memory, cross-device continuity, and optional collaboration as layered capabilities.

## Product thesis

**Cairn remembers the work so people and agents do not have to.** The AI manages relevance, compression, retention, and retrieval. The person owns the memory and controls privacy, correction, deletion, and sharing. Each roadmap layer must produce a usable Cairn rather than requiring the complete platform before any value appears.

## Documentation

Start with the [documentation index](docs/README.md). The documents are deliberately marked `draft` until they have been reviewed and accepted.

| Document | Purpose |
| --- | --- |
| [Product](docs/product.md) | AI-managed personal memory, ownership model, scope, and success signals |
| [Roadmap](docs/roadmap.md) | Layered milestones and outcome-based gates |
| [Architecture](docs/architecture.md) | Proposed edge, sync, API, storage, search, and web boundaries |
| [Policy](docs/policy.md) | Automatic memory lifecycle, ownership, privacy, correction, and sharing rules |
| [Phase 0 evidence](docs/evidence.md) | Recorded baseline artifacts, source snapshot, checks, and limitations |
| [AgentMemory baseline](docs/contracts/agentmemory-baseline.md) | Upstream compatibility inventory and migration classification |
| [ADR 0001](docs/adr/0001-compatibility-first-rewrite.md) | Why the rewrite uses an incremental compatibility migration |
| [ADR 0002](docs/adr/0002-collaborative-sync-model.md) | Why collaboration uses scoped spaces and event-based synchronization |
| [ADR 0003](docs/adr/0003-server-authoritative-postgres.md) | Server authority, PostgreSQL, embeddings, and search storage |
| [ADR 0004](docs/adr/0004-trunk-based-development-and-release.md) | Branch, worktree, CI, and GitHub release workflow |
| [ADR 0005](docs/adr/0005-phase-0-product-boundaries.md) | Phase 0 personal-memory boundaries and unresolved decisions |
| [Collaborative memory specification](docs/specs/collaborative-memory.md) | Behavior and boundaries for personal sync, project promotion, and cross-person memory |
| [Memory context and promotion](docs/specs/memory-context-and-promotion.md) | Scope escalation, branch/worktree context, and context packing |
| [Graph-assisted retrieval](docs/specs/graph-assisted-retrieval.md) | Graph RAG, embeddings, hybrid retrieval, and code context |
| [Documentation standards](docs/standards.md) | How project documentation is created, reviewed, and changed |

## Target stack

- **Backend:** Rust server, REST, MCP, background jobs, authentication, and durable persistence.
- **Frontend:** Next.js App Router for the dashboard and product-facing web surfaces.
- **UI:** shadcn/ui components with a separate console design system from the marketing site.
- **Storage:** PostgreSQL as the server source of truth, pgvector and PostgreSQL full-text search for retrieval, plus an optional local edge spool/cache.

## Project status

Phase 0's product, contract, policy, fixture, and golden-corpus artifacts are present. The local Layer 1 and Layer 2 implementation gates, the trusted-device sync slice, the first self-hosted account-authentication adapter, the PostgreSQL Layer 3 boundary, and the first Layer 4 project-promotion/merge slice are complete. Layer 5 project sharing, teams, membership, maintenance, and the `apps/web` console are implemented. Federation remains later work. File-store authorization and sync deletion behavior are covered by the focused tests for those paths; they are not a separate finished product claim.

## Agent CLI and MCP

People use the web console. Agents use the `cairn` CLI, the hooks, and `cairn mcp`, all of which call the server's REST API with a device token. Start a server, create the first account and a device token in the web console, then:

```bash
cargo run -p cairn-server -- --store /tmp/cairn-memory.json --bind 127.0.0.1:3111
printf %s "<device-token>" | cargo run -p cairn-client --bin cairn -- setup http://127.0.0.1:3111
cargo run -p cairn-client --bin cairn -- start session-1 cairn-demo main worktree-1 abc1234
cargo run -p cairn-client --bin cairn -- capture session-1 event-1 "The session middleware validates the signed cookie before loading the user."
cargo run -p cairn-client --bin cairn -- end session-1
cargo run -p cairn-client --bin cairn -- context "signed cookie" 5
cargo run -p cairn-client --bin cairn -- forget event-1
cargo run -p cairn-client --bin cairn -- export /tmp/cairn-memory-export.json
```

`setup` reads the token from stdin (a hidden prompt on a terminal, so it stays out of the process list and shell history) and stores the URL and token in `$XDG_CONFIG_HOME/cairn/config.json` (else `~/.config/cairn/config.json`) with mode `0600`; `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` together replace it, and setting only one is an error. `export` writes its bundle with mode `0600`. The URL must be `https`, or `http` for a loopback host. Account and device management stay in the web console; a device token gets `403` there. The tool and command reference is [CM-CONTRACT-002](docs/contracts/mcp-layer1.md).

The first REST compatibility server is:

```bash
cargo run -p cairn-server -- --store /tmp/cairn-memory.json --bind 127.0.0.1:3111
```

The server-authoritative PostgreSQL Layer 3 boundary can be started with:

```bash
cargo run -p cairn-server -- --database-url "$CAIRN_DATABASE_URL" --bind 127.0.0.1:3111
```

This mode runs account, trusted-device, authenticated sync, session summary,
explicit memory, corrections, context retrieval, lifecycle diagnostics,
export/import, audit history, and keyword search routes from `cairn-storage`.
It applies the Layer 3 migrations on startup.

It serves health, readiness, self-hosted account bootstrap/login/logout/current-account, account-authorized device registration, project registration, branch/worktree registration, explicit promotion, merge confirmation, branch closure, project verification, session lifecycle, observation capture, search, context, resume, unresolved-work, why-changed, forget, smart-search, recall, explicit remember, correction, export/import, and lifecycle diagnostics under `/cairn-memory/`. It currently proves bounded capture, secret redaction, idempotent event capture, lifecycle classification, deterministic session compression, lessons, corrections, retention sweep, evidence-backed session summaries, branch-aware scopes, structured code context, deterministic code relationships, stale and contradiction signals, keyword recall, context packing, forget suppression, export/import, restart persistence, and account/device ownership checks.

The local viewer is available at:

```text
http://127.0.0.1:3111/viewer
```

It is an exception-driven control surface for recent captures, durable memories, provenance, lifecycle state, diagnostics, search, and forget—not a manual knowledge-base editor.

The MCP server runs over stdio as a proxy to the same REST API, using the configured device token:

```bash
cargo run -p cairn-client --bin cairn -- mcp
```

It supports the Layer 1/2 memory tools over newline-delimited JSON-RPC, including session capture, consolidation, recall, resume, unresolved-work, why-changed, explicit save, lifecycle diagnostics, forget, correction, export, and import. The export format is versioned and idempotently restorable. The first Next.js console is `apps/web`. The embedded viewer at `/viewer` remains.

```bash
cd apps/web
CAIRN_API_URL=http://127.0.0.1:3111 npm run dev
```

`CAIRN_API_URL` defaults to `http://127.0.0.1:3111`. `./scripts/smoke_web.sh` checks login, the project list, and a member's shared read.

For automatic hook capture, send one JSON object to the portable adapter:

```bash
printf '%s\n' '{"sessionId":"session-1","content":"The test passed.","role":"tool","source":"PostToolUse"}' \
  | python3 scripts/cairn_hook.py
```

The adapter needs `CAIRN_DEVICE_TOKEN` (a device token from the web console) and posts `sync/push` to `CAIRN_URL` (default `http://127.0.0.1:3111`); Cairn handles redaction, lifecycle classification, persistence, and duplicate delivery. A reused event id with different content is a `409`.

To verify the complete local application flow with real server processes and temporary stores:

```bash
./scripts/smoke_full_flow.sh
```

This simulates hook capture, duplicate delivery, session consolidation, secret redaction, restart recovery, correction, forget, export/import, and viewer availability without requiring PostgreSQL or an external model provider.

For Layer 2 branch-aware continuity, the same flow also verifies that project
and global memories remain available while a worktree-local hypothesis stays
out of an unrelated `main` context. It also verifies deterministic resume and
unresolved-work retrieval from the session summary, plus structured files,
symbols, commands, tests, and services context.

The Layer 2 measurement gate is:

```bash
python3 scripts/measure_layer2.py
```

It checks why-changed evidence, branch isolation, stale and contradictory
signals, and repeated-investigation stability against the Layer 2 golden corpus.

The first Layer 3 trusted-device acceptance flow is:

```bash
./scripts/smoke_layer3.sh
```

It verifies authenticated outbox replay, cross-device pull, cursor resume,
duplicate delivery, token rotation, last-seen state, secret redaction, and
revocation against the local compatibility path. PostgreSQL adapter execution
requires a fresh database and can be checked with:

```bash
CAIRN_TEST_DATABASE_URL="$CAIRN_DATABASE_URL" \
  cargo test -p cairn-storage --test postgres -- --ignored --nocapture
```

The HTTP boundary can be exercised against the same kind of fresh database with:

```bash
CAIRN_DATABASE_URL="$CAIRN_DATABASE_URL" ./scripts/smoke_postgres.sh
```

## Reference

- Inspiration: [rohitg00/agentmemory](https://github.com/rohitg00/agentmemory)
- Documentation template: [cunilab/Document-Template-for-Product-Development](https://github.com/cunilab/Document-Template-for-Product-Development)

## Development with Docker

An isolated stack (PostgreSQL 16, server built from source, Next.js console) runs
under the Compose project `cairn-dev`, bound to 127.0.0.1 only:

```sh
./scripts/dev_up.sh          # build, start, wait until healthy; creates .env.dev
./scripts/dev_down.sh        # stop, keep the database volume
./scripts/dev_down.sh --wipe # stop and drop the database volume
```

| Service  | URL / address                                  |
| -------- | ---------------------------------------------- |
| web      | http://127.0.0.1:53000/login                   |
| server   | http://127.0.0.1:58080/cairn-memory/health     |
| postgres | 127.0.0.1:55432 (user `cairn`, db `cairn`)     |

`.env.dev` holds a generated dev-only database password and is git-ignored.
Images are rebuilt from the working tree on each `dev_up.sh`; there is no hot
reload.
