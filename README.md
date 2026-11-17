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

Phase 0's product, contract, policy, fixture, and golden-corpus artifacts are present. The local Layer 1 and Layer 2 implementation gates, the trusted-device sync slice, the first self-hosted account-authentication adapter, the PostgreSQL Layer 3 boundary, and the first Layer 4 project-promotion/merge slice are complete. The shared project model, membership, owner transfer and the `apps/web` console are under validation on this branch. Federation remains later work. File-store authorization and sync deletion behavior are covered by the focused tests for those paths; they are not a separate finished product claim.

## Install and update

Releases are tags on `main` published as GitHub Releases ([release process](docs/release.md)). Each release holds raw CLI binaries for `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` (static, any Linux distribution), and `x86_64-pc-windows-msvc`. Server binaries use the same macOS/Windows targets and Linux `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu`: ONNX Runtime requires glibc, so use a system compatible with Ubuntu 24.04 (glibc 2.39 or newer), or the Debian trixie slim server image. Alpine/musl server binaries are unsupported. Assets are named `cairn-<target>[.exe]` and `cairn-server-<target>[.exe]`, with hook scripts and `SHA256SUMS`. Download the `cairn` asset for your platform, check it against `SHA256SUMS`, make it executable, and put it on your `PATH` as `cairn`:

```bash
cairn --version
cairn update --check   # report only
cairn update           # install the latest release and refresh the hook scripts
```

`cairn update` verifies every download against the release's `SHA256SUMS` and replaces the running executable; if its directory is not writable it says so and changes nothing. `cairn setup` and `cairn update` also install `cairn_hook.py`, `cairn_outbox.py` and `cairn_instructions.txt` of the release that matches the installed binary into `hook/` beside the config file (see [examples](examples/README.md)); a script you edited is kept as `<name>.bak` and the command says so. Before swapping, the new binary must run `--version` and report the expected version. Interactive commands print a one-line notice, at most one check per day, when a newer release exists; set `CAIRN_NO_UPDATE_CHECK=1` to turn it off. `cairn mcp` and the hooks never check or update. Limits and the trust model: [ADR 0012](docs/adr/0012-github-releases-and-self-update.md).

## Agent CLI and MCP

People use the web console. Agents use the `cairn` CLI, the hooks, and `cairn mcp`, all of which call the server's REST API with a device token. Start a server, create the first account and a device token in the web console, then:

```bash
cargo run -p cairn-server -- --store /tmp/cairn-memory.json --bind 127.0.0.1:3111
printf %s "<device-token>" | cargo run -p cairn-client --bin cairn -- setup http://127.0.0.1:3111
cargo run -p cairn-client --bin cairn -- start session-1
cargo run -p cairn-client --bin cairn -- capture session-1 event-1 "The session middleware validates the signed cookie before loading the user."
cargo run -p cairn-client --bin cairn -- end session-1
cargo run -p cairn-client --bin cairn -- context "signed cookie" 5
cargo run -p cairn-client --bin cairn -- forget event-1
cargo run -p cairn-client --bin cairn -- export /tmp/cairn-memory-export.json
```

For an installed agent, opt in to its hooks/instructions and MCP registration:

```sh
printf %s "<device-token>" | cairn setup https://cairn.example.test --agent claude
printf %s "<device-token>" | cairn setup https://cairn.example.test --agent codex
printf %s "<device-token>" | cairn setup https://cairn.example.test --agent opencode
cairn setup https://cairn.example.test --agent codex --remove
```

Python 3 is required; Codex/OpenCode setup also needs that agent's installed CLI.
Plain URL-only setup keeps its existing behavior. Repeating setup/removal preserves
unrelated agent config; removal reads no token and keeps the Cairn credentials.
Claude Code and Codex receive a short start note plus a bounded data-fenced brief,
compact save instructions once after first work, and a guarded Stop request only
after new work. In Codex, review/trust the hooks through `/hooks` before they run.
OpenCode V2 gets a marked global `AGENTS.md` block and MCP registration, using its
native CLI to preserve JSONC comments; this adapter has no automatic hook brief or
Stop request. [Capabilities, official references and limits](docs/contracts/hook-v1.md#agent-instructions-and-setup-v014).

`setup` reads the token from stdin (a hidden prompt on a unix terminal, so it stays out of the process list and shell history; on Windows, or where `stty` is missing, a terminal prompt is visible and `setup` warns, so pipe the token instead) and stores the URL and token in `$XDG_CONFIG_HOME/cairn/config.json` (else `~/.config/cairn/config.json`) with mode `0600`; `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` together replace it, and setting only one is an error. `export` writes its bundle with mode `0600`. The URL must be `https`, or `http` for a loopback host; it may include a path prefix (`https://host/agent`) when a reverse proxy mounts the API there. A hosted layout is in [deploy](docs/deploy.md). Account and device management stay in the web console; a device token gets `403` there. The tool and command reference is [CM-CONTRACT-002](docs/contracts/mcp-layer1.md).

To work together, run `cairn init "Project name"` in a fresh checkout and commit
`.cairn/project.json`. It contains `project_id`, the normalized `host/org/repo`
`repository`, and the display `name`, without a secret or server URL. Omit the name
to default to the repository's short name. Running `cairn init` with an existing
file joins the same project without renaming it; older files without `name` gain
the server's name. A different supplied name or a file/server name mismatch is
refused. The owner can explicitly repair or change both names with
`cairn init --rename "Project name"` in that checkout, then commit the updated file.
That also fixes a project accidentally named `--help` by an older CLI after the
server and CLI are upgraded to 0.1.4. Captures also join by the stable ID.
`cairn init --help`, `cairn init -h`, `cairn --help` and `cairn help init` print
usage without connecting; unknown options and flag-like positional values are
rejected before any request. See the [project contract](docs/contracts/project-layer4.md#init-and-membership).
Repositories without the file create no project: captures and automatic work
notes remain private under `personal`, marked as work notes rather than personal
behaviour preferences. Broken/unknown files warn while private capture continues.

Memories have three explicit scopes: **project** (all members, including initialized-repository captures and summaries), **personal** (private behaviour and preferences), and **global** (all server users; author/admin edits). There is no personal-per-repository scope or promote step. Redaction runs before storage; project members can read raw captures.

Agents save a structured record with required title, summary, reason and scope:

```sh
cairn save '{"scope":"personal","kind":"preference","title":"Concise answers","summary":"Use short paragraphs.","reason":"Reduce reading time.","preference":"concise"}'
# Or pipe one JSON record to cairn save / cairn lesson.
cairn memories
cairn memory memory-id
```

Common fields and typed decision/rule/gotcha/procedure/fact/preference fields are available through CLI, MCP and REST, and visible in the console's Memories list/detail. See [the record contract](docs/contracts/project-layer4.md#memory-scopes-and-record). Existing lexical recall searches title, summary, details and reason; later ADR 0014 stages are still planned. Project owners/admins can transfer ownership to a member. Project identity and shared records are read online; personal export/import is not a project backup. Development stores require a reset for this pre-release schema.

**Joining trusts possession of the project ID.** Any admin-created account on the
configured server with a copied/leaked ID can join permanently, read all decisions
and see member emails. Cairn does not prove Git access; removing repository access
or the file does not remove membership. Keep the server configuration consistent
and revoke device credentials separately from web-session password resets. See
[ADR 0013](docs/adr/0013-shared-project-decisions-and-personal-memory-scopes.md) and
[the project contract](docs/contracts/project-layer4.md) for the trust boundary and
current routes. Branch status is personal bookkeeping; the console says "Record merge".

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
export/import, audit history, and hybrid search routes from `cairn-storage`.
It applies the Layer 3 migrations on startup.

It serves health, readiness, self-hosted account bootstrap/login/logout/current-account, account-authorized device registration, project registration, branch/worktree registration, scoped memory saves, merge confirmation, branch closure, project verification, session lifecycle, observation capture, search, context, resume, unresolved-work, why-changed, forget, smart-search, recall, explicit remember, correction, helpful/stale feedback, export/import, and lifecycle diagnostics under `/cairn-memory/`. It currently proves bounded capture, secret redaction, idempotent event capture, lifecycle classification, deterministic session compression, lessons, corrections, retention sweep, usage retention scoring, evidence-backed session summaries, branch-aware scopes, structured code context, deterministic code relationships, stale and contradiction signals, hybrid recall, context packing, forget suppression, export/import, restart persistence, and account/device ownership checks.

Hybrid recall combines full text, code entities, one-hop evidence links and local
MiniLM embeddings with reciprocal-rank fusion, type weights and pinned boosts.
Results keep project/personal/global visibility labels. PostgreSQL requires
pgvector (`pgvector/pgvector:pg17` in development/CI). Production server images
include the pinned Apache-2.0 model. For a standalone server, provision once:

```sh
python3 scripts/download_embedding_model.py /path/to/minilm
cargo run -p cairn-server -- --database-url "$CAIRN_DATABASE_URL" --embedding-model-dir /path/to/minilm
```

The directory can also be set with `CAIRN_EMBEDDING_MODEL_DIR`. The server
verifies local files and makes no model network calls. Health reports
`semantic_search: enabled` when loaded; missing or invalid files leave lexical,
entity and link retrieval available. Embedding and session distillation share a
durable job table. The worker polls every 2 seconds, claims a job for 120
seconds, and allows five attempts. A transient failure waits 30, 60, 120, then
240 seconds; the fifth failure quarantines. A crashed worker's lease can be reclaimed. Jobs that will not
succeed sit in quarantine until an admin requeues or discards them on the Jobs
page. Succeeded jobs are removed after 24 hours. Searches do not wait for
indexing. Operators with an admin web session can read pending, running and
quarantined counts at `GET /cairn-memory/admin/jobs`. See the
[jobs contract](docs/contracts/continuity-v1.md#durable-jobs-v014) and the
[retrieval contract](docs/contracts/continuity-v1.md#hybrid-recall-v014).

The local viewer is available at:

```text
http://127.0.0.1:3111/viewer
```

The embedded viewer inspects personal memory; project actions are in the web console.

The MCP server runs over stdio as a proxy to the same REST API, using the configured device token:

```bash
cargo run -p cairn-client --bin cairn -- mcp
```

It supports the Layer 1/2 memory tools over newline-delimited JSON-RPC, including session capture, consolidation, recall, resume, unresolved-work, why-changed, explicit save, lifecycle diagnostics, forget, correction, export, and import. The export format is versioned and idempotently restorable. The first Next.js console is `apps/web`. The embedded viewer at `/viewer` remains.

```bash
cd apps/web
CAIRN_API_URL=http://127.0.0.1:3111 npm run dev
```

`CAIRN_API_URL` defaults to `http://127.0.0.1:3111`. `./scripts/smoke_web.sh` checks login, the project list, and the common project decision list (the member joins by a capture that carries the project id).

For automatic hook capture, send one JSON object to the portable adapter:

```bash
printf '%s\n' '{"sessionId":"session-1","content":"The test passed.","role":"tool","source":"PostToolUse"}' \
  | python3 scripts/cairn_hook.py
```

The adapter needs `CAIRN_DEVICE_TOKEN` (a device token from the web console) and posts `sync/push` to `CAIRN_URL` (default `http://127.0.0.1:3111`, an optional path prefix such as `https://host/agent` is kept); Cairn handles redaction, lifecycle classification, persistence, and duplicate delivery. A reused event id with different content is a `409`.

Assistant text is skipped by default; opt in with `CAIRN_CAPTURE_ASSISTANT=1`.
Place a regular `.cairnignore` file in a folder to skip it and its descendants.
Lookup stops at the Git root, or filesystem root outside Git. Skips make no
network or outbox writes. The hook scrubs secrets before truncating content;
see the [hook contract](docs/contracts/hook-v1.md#capture-hygiene-v014).

Session end (including idle close) saves an evidence-linked session note,
separate handoff and explicitly labelled decision/gotcha/procedure/rule/fact
notes, without an LLM. Re-ending does not duplicate them. Project captures
produce project notes; unattached or mixed work stays private. The exact
[distillation grammar and limits](docs/contracts/continuity-v1.md#session-end-distillation-v014)
explain which fields the rules can fill without guessing.

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
reload. The web image uses Next.js standalone output on a slim Node runtime
(about 330 MB) and reads `CAIRN_API_URL` at runtime.
