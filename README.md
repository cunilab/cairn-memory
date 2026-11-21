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
cairn update           # install the latest release and refresh the hook commands
```

`cairn update` verifies every download against the release's `SHA256SUMS` and replaces the running executable; if its directory is not writable it says so and changes nothing. The hook is the `cairn hook` command of the same binary, so there are no scripts to keep in step: after replacing the executable `cairn update` rewrites the hook commands of every agent that already has a Cairn hook (see [examples](docs/examples/README.md)). Before swapping, the new binary must run `--version` and report the expected version. Interactive commands print a one-line notice, at most one check per day, when a newer release exists; set `CAIRN_NO_UPDATE_CHECK=1` to turn it off. `cairn mcp` and the hooks never check or update. Limits and the trust model: [ADR 0012](docs/adr/0012-github-releases-and-self-update.md).

## Agent CLI and MCP

People use the web console. Agents use the `cairn` CLI, the hooks, and `cairn mcp`, all of which call the server's REST API with a device token. Start a server, create the first account and a device token in the web console, then:

```bash
cargo run -p cairn-server -- --database-url "$CAIRN_DATABASE_URL" --bind 127.0.0.1:3111
printf %s "<device-token>" | cargo run -p cairn-client --bin cairn -- setup http://127.0.0.1:3111
cargo run -p cairn-client --bin cairn -- save '{"kind":"fact","scope":"personal","title":"Session cookie","summary":"The session middleware validates the signed cookie before loading the user.","reason":"read in the auth code","fields":{"statement":"cookie checked first","source":"auth.rs"}}'
cargo run -p cairn-client --bin cairn -- search "signed cookie" --limit 5
cargo run -p cairn-client --bin cairn -- forget <memory-id>
```

For an installed agent, opt in to its hooks/instructions and MCP registration:

```sh
printf %s "<device-token>" | cairn setup https://cairn.example.test --agent claude
printf %s "<device-token>" | cairn setup https://cairn.example.test --agent codex
printf %s "<device-token>" | cairn setup https://cairn.example.test --agent opencode
cairn setup https://cairn.example.test --agent codex --remove
```

Codex/OpenCode setup also needs that agent's installed CLI. Setup writes
`cairn hook <Event> --agent <agent> --config <file> --cairn-managed` commands (no Python),
replaces older Cairn-managed entries, and does not need the network. Repeating setup/removal preserves
unrelated agent config; removal reads no token and keeps the Cairn credentials.
Claude Code and Codex receive the bounded data-fenced brief at session start and after
compaction; their prompts, edits and commands are captured, and a checkpoint is saved at Stop
(and, for Claude, before compaction). In Codex, review/trust the hooks through `/hooks` before they run.
OpenCode V2 gets a marked global `AGENTS.md` block, the MCP registration (through its native
CLI, which keeps JSONC comments) and a plugin that runs `cairn hook`.
[Hook events, setup and limits](docs/contracts/hook-v1.md#setup).

`setup` reads the token from stdin (a hidden prompt on a unix terminal, so it stays out of the process list and shell history; on Windows, or where `stty` is missing, a terminal prompt is visible and `setup` warns, so pipe the token instead) and stores the URL and token in the selected config file with mode `0600`. `CAIRN_CONFIG` selects that file; otherwise it is `$XDG_CONFIG_HOME/cairn/config.json` (else `~/.config/cairn/config.json`, else `%APPDATA%`). `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are ignored, with one stderr warning that does not print their values. `export` writes its bundle with mode `0600`. The URL must be `https`, or `http` for a loopback host; it may include a path prefix (`https://host/agent`) when a reverse proxy mounts the API there. A hosted layout is in [deploy](docs/deploy.md). Account and device management stay in the web console; a device token gets `403` there. The tool and command reference is [CM-CONTRACT-002](docs/contracts/mcp-layer1.md).

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

Memories have three explicit scopes: **project** (all members, including initialized-repository captures and summaries), **personal** (private behaviour and preferences), and **global** (all server users; author/admin edits). There is no personal-per-repository scope or promote step. Redaction runs before storage; raw captures are kept for 30 days for distillation and are never recalled or shown.

Agents save a structured record with required title, summary, reason and scope:

```sh
cairn save '{"scope":"personal","kind":"preference","title":"Concise answers","summary":"Use short paragraphs.","reason":"Reduce reading time.","preference":"concise"}'
# Or pipe one JSON record: cairn save - < record.json
cairn search "job queue" --kind decision --limit 3
cairn get memory-id
cairn feedback memory-id helpful
cairn forget memory-id
```

The agent CLI is `setup`, `init`, `update`, `doctor`, `mcp`, `hook`, `version` and the five verbs `search`, `get`, `save`, `feedback`, `forget`; the same five are the only MCP tools. `cairn doctor` checks config, server, device token, project file and agent wiring without writing anything. Contract: [docs/contracts/mcp-layer1.md](docs/contracts/mcp-layer1.md).

Common fields and typed decision/rule/gotcha/procedure/fact/preference fields are available through CLI, MCP and REST, and visible in the console's Memories list/detail. See [the record contract](docs/contracts/project-layer4.md#memory-scopes-and-record). Existing lexical recall searches title, summary, details and reason; later ADR 0014 stages are still planned. Project owners/admins can transfer ownership to a member. Project identity and shared records are read online; export is not a project backup. Development stores require a reset for this pre-release schema.

**Joining trusts possession of the project ID.** Any admin-created account on the
configured server with a copied/leaked ID can join permanently, read all decisions
and see member emails. Cairn does not prove Git access; removing repository access
or the file does not remove membership. Keep the server configuration consistent
and revoke device credentials separately from web-session password resets. See
[ADR 0013](docs/adr/0013-shared-project-decisions-and-personal-memory-scopes.md) and
[the project contract](docs/contracts/project-layer4.md) for the trust boundary and
current routes. The branch list is derived from the branch each project memory was saved on.

The server runs on PostgreSQL with pgvector only. `CAIRN_DATABASE_URL` is a database the server may
migrate; everything lives in its schema `cairn` (`--schema` picks another one). The server applies
`migrations/` on start and refuses a database left by 0.1.5 (`pre-0.1.6 database; recreate it`).

```bash
cargo run -p cairn-server -- --database-url "$CAIRN_DATABASE_URL" --bind 127.0.0.1:3111
```

It serves health, self-hosted accounts, trusted devices, projects (init, rename, ownership, members, derived branches and conflicts), session capture, typed memory saves (`/remember`: a decision, rule, gotcha, procedure, fact, preference or note with per-kind `fields`, `supersedes` and `contradicts`), search, get, helpful/wrong feedback, forget, the readable memory graph, export, diagnostics and the admin job queue under `/cairn-memory/`. Background jobs distill sessions into notes and a handoff, embed memories, and consolidate near-duplicates automatically (cosine 0.92 or more supersedes the older memory; 0.75 to 0.92 adds a `relates` link). A correction is a save with `supersedes`.

Hybrid recall combines full text, code entities, one-hop evidence and typed memory links, and local
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
succeed sit in quarantine; Cairn retries them once at the first start after a server
upgrade and again after 24 hours (nobody requeues by hand). Succeeded jobs are removed after 24 hours. Searches do not wait for
indexing. Operators with an admin web session can read pending, running and
quarantined counts at `GET /cairn-memory/admin/jobs`. See the
[jobs contract](docs/contracts/continuity-v1.md#durable-jobs-v014) and the
[retrieval contract](docs/contracts/continuity-v1.md#hybrid-recall-v014).

The MCP server runs over stdio as a proxy to the same REST API, using the configured device token:

```bash
cargo run -p cairn-client --bin cairn -- mcp
```

It supports five memory tools over newline-delimited JSON-RPC: search, get, save, feedback and forget. The Next.js console is `apps/web`.

```bash
cd apps/web
CAIRN_API_URL=http://127.0.0.1:3111 npm run dev
```

`CAIRN_API_URL` defaults to `http://127.0.0.1:3111`. `./scripts/smoke_web.sh` checks login, the project list, and the common project decision list (the member joins by a capture that carries the project id).

Automatic capture is `cairn hook <Event>`, which `cairn setup --agent` installs into the agent. To try it, send one event on stdin:

```bash
printf '%s' '{"session_id":"s1","cwd":"'"$PWD"'","prompt":"fix the retry bug"}' \
  | cairn hook UserPromptSubmit --agent claude
```

The hook reads the server URL and device token from the selected config file. An unmanaged hook uses `--config`, else `CAIRN_CONFIG`, else the default file. An installed `--cairn-managed` hook uses a non-empty `CAIRN_CONFIG` when set, otherwise its setup-time `--config`. It posts typed captures to `observe`; a path prefix such as `https://host/agent` is kept, and a capture that cannot be delivered waits in a device-bound outbox. The server redacts again, deduplicates, and answers `409` for a reused event id with different content.

Assistant text is skipped by default; opt in with `CAIRN_CAPTURE_ASSISTANT=1`.
Place a regular `.cairnignore` file in a folder to skip it and its descendants.
Lookup stops at the Git root, or filesystem root outside Git. Skips make no
network or outbox writes. The hook scrubs secrets before truncating content;
see the [hook contract](docs/contracts/hook-v1.md#events).

Session end (including idle close) saves one handoff and explicitly labelled
decision/gotcha/procedure/rule/fact notes, without an LLM. Re-ending does not duplicate them. Project captures
produce project notes; unattached or mixed work stays private. The exact
[distillation grammar and limits](docs/contracts/continuity-v1.md#session-end-distillation-v014)
explain which fields the rules can fill without guessing.

The tests and smokes need a PostgreSQL database with pgvector, for example
`docker run -d -p 127.0.0.1:55432:5432 -e POSTGRES_PASSWORD=pw pgvector/pgvector:pg17`:

```bash
export CAIRN_TEST_DATABASE_URL=postgres://postgres:pw@127.0.0.1:55432/postgres   # or: ./scripts/dev_up.sh && source scripts/dev_env.sh
cargo test --workspace            # each test gets its own schema; set CAIRN_TEST_REQUIRE_DB=1 to fail instead of skip
./scripts/smoke_mcp.sh
./scripts/smoke_web.sh
```

Add `CAIRN_TEST_EMBEDDING_MODEL_DIR=<pinned MiniLM directory>` to run the semantic tests. The smoke
scripts start real server processes, each in a schema of its own, and do not load the embedding model:
without `--embedding-model-dir` the server keeps lexical, entity, and link recall.

## Benchmark harness

`scripts/bench.py` measures the agent-facing token cost and recall quality against a disposable
`pgvector/pgvector:pg17` container and a synthetic corpus (`bench/fixtures/`, no real memories). It
reports MCP surface size, bytes injected by the hooks in a scripted session, recall@5 and MRR, bytes
per call, and zero-hit behaviour. Needs Docker, Python 3 and a built workspace; the semantic channel
uses the pinned MiniLM model when `scripts/download_embedding_model.py ~/.cache/cairn-bench/minilm`
has been run (otherwise pass `--lexical-only`).

```bash
python3 scripts/bench.py --out bench/result.json --label my-change
```

`--min-hit5 0.68` exits 1 when `memory_search` hit@5 is below the floor; CI runs it as the `Bench` job. The pre-rewrite baseline is `bench/baseline-0.1.6-pre.json` and the 0.1.6 step 2 reference is `bench/after-0.1.6-step2.json` (hit@5 0.72, zero-hit 4/5); see `docs/evidence.md` (2026-11-21).

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
