---
id: CM-REL-001
title: "Cairn Memory Release Process"
status: draft
---

# Cairn Memory Release Process

How changes reach `main`, how a release is cut, and how installed clients update. The decision and its security limits are in [ADR 0012](./adr/0012-github-releases-and-self-update.md); the branch model is [ADR 0004](./adr/0004-trunk-based-development-and-release.md). The workflows are `.github/workflows/ci.yml` and `.github/workflows/release.yml`; this document describes them and does not claim they have run until a release exists.

## Trunk-based flow

- `main` is the trunk and is always releasable. Nobody pushes to it directly.
- Work happens on a short-lived branch named `feat/`, `fix/`, `docs/`, or `chore/` plus a short name (for example `fix/CM-123-outbox-retry`), merged by pull request and then deleted.
- A pull request needs every required check green. The required checks are the CI job names:
  - `Rust format and clippy` (also checks that `cairn-core` has no `sqlx`, `tokio`, `axum` or `reqwest` dependency)
  - `Rust tests` (a `pgvector/pgvector:pg17` service and the pinned MiniLM model; `CAIRN_TEST_REQUIRE_DB=1`, so a missing database fails instead of skipping)
  - `Build on macos-15` and `Build on windows-latest`
  - `Web console typecheck, lint, build`
  - `Documentation validation`
  - `Acceptance smokes` (`scripts/smoke_mcp.sh` and `scripts/smoke_web.sh`)
  - `Docker images build` (builds `docker/server.Dockerfile` and `docker/web.Dockerfile` without pushing, so an image breaks in the pull request, not at tag time)
  - `Bench` (`scripts/bench.py` on bench v2 with 2,000 distractors, `--min-hit5 0.60 --max-weak-hits 0`, then on v1, `--min-hit5 0.88`; uploads the result JSON; the 0.1.7 run is hit@5 0.674, see ADR 0015)
- Renaming a job in `ci.yml` detaches it from the branch protection rule; update the rule in the same change.
- The acceptance smokes do not download the embedding model. They use the server's lexical, entity, and link recall. The pinned model stays on the Rust tests and Bench jobs.

## Cut a release

Releases are tags `vMAJOR.MINOR.PATCH` on `main`. The version lives once, in `[workspace.package]` of the root `Cargo.toml`.

1. On a branch (for example `chore/release-0.2.0`), change `version` in `[workspace.package]`, set the same `version` in `apps/web/package.json` and in the two places of `apps/web/package-lock.json` (`scripts/check_web_version.py` fails the documentation check when they differ), and let `Cargo.lock` follow (`cargo check`). Put migration notes, upgrade guidance, and compatibility or rollback notes for operators in the pull request description (ADR 0004 requires them with a release).
2. Open the pull request, wait for the required checks, merge it.
3. Tag the merge commit on `main` and push the tag:

   ```bash
   git switch main && git pull --ff-only
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. The `Release` workflow then:
   - fails unless the tag equals the workspace version, is `MAJOR.MINOR.PATCH` (no pre-release suffix), and points at a commit on `main`;
   - builds the CLI natively for `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, and `x86_64-pc-windows-msvc`; the server uses the same macOS/Windows targets and `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu` on Linux, because ONNX Runtime requires glibc;
   - creates the GitHub Release with generated notes prefixed by server runtime/upgrade requirements and these assets: `cairn-<target>[.exe]`, `cairn-server-<target>[.exe]`, and `SHA256SUMS` covering every other asset. Binaries are raw files, not archives;
   - after the release exists, pushes the container images `ghcr.io/cunilab/cairn-memory-server` and `ghcr.io/cunilab/cairn-memory-web` for `linux/amd64` and `linux/arm64`, tagged `vMAJOR.MINOR.PATCH` and `latest` (each platform is built natively, then merged into one multi-arch manifest). How to run them: [deploy](./deploy.md).
5. A bad release is not edited in place: delete the release and tag, fix on a branch, and release the next patch version. Clients never install a version lower than their own.

Only the `Publish release` job has `contents: write`, and only the image and manifest jobs have `packages: write`.

## 0.1.4 upgrade notes

Upgrade the server and CLI together. This patch adds offline CLI help and rejects
flag-like positional arguments before requests; `cairn init --help` can no longer
create a project. Init writes the display name to `.cairn/project.json`, reads
older files without it, and refuses name conflicts. The owner can repair an
accidental `--help` name with `cairn init --rename "Project name"` in the affected
checkout, then commit its updated file. This uses a new owner-only REST route and
records an audit event; see the [project contract](contracts/project-layer4.md).
Older CLI/hook readers tolerate the added project-name field. A downgrade loses
the rename route and restores the old CLI argument bug; use 0.1.4 for init/rename.

The memory-scope change requires fresh pre-release stores: reset older databases
and file stores before using 0.1.4, because old private `global` notes must never
be reinterpreted as server-wide notes. See [ADR 0014](adr/0014-memory-scopes-distillation-and-hybrid-recall.md).
PostgreSQL now requires pgvector and migration `0014_hybrid_embeddings.sql`.
The server adds `fastembed`/CPU ONNX Runtime and a pinned Apache-2.0 MiniLM model;
the CLI remains static musl on Linux. Linux server release assets are GNU binaries
built on Ubuntu 24.04: use a compatible glibc system (2.39 or newer), with the C++
runtime installed, rather than Alpine/musl. The exact minimum runtime version has
not been measured. The `linux/amd64` and `linux/arm64` production images build on
Debian trixie and run on `debian:trixie-slim`, with the C++ runtime and local
model included. Standalone servers provision the model using the pinned download
script and `--embedding-model-dir` / `CAIRN_EMBEDDING_MODEL_DIR`; no model means
lexical/entity/link fallback. See [hybrid recall](contracts/continuity-v1.md#hybrid-recall-v014).

## 0.1.5 upgrade notes

Upgrade the server and CLI together. A 0.1.5 hook calls
`POST /cairn-memory/session/checkpoint`, which a 0.1.4 server does not serve.
Capture hiding and the relevance floor are server behavior; compact MCP cards
are the CLI view of the same routes. REST and CLI commands still print server
JSON.

This release adds no SQL file and does not change `STORE_VERSION` or
`EXPORT_VERSION`. Startup still applies the existing migrations, ending at
`0018_memory_links.sql`. PostgreSQL still needs pgvector, and the glibc, ONNX,
and MiniLM requirements in the 0.1.4 section above still apply. A store that
already completed the 0.1.4 scope change does not need a reset. The pre-0.1.4
fresh-store warning in that section still applies to anything older.

`cairn setup <server-url>` writes `url` and `token` to the selected file. A
non-empty `CAIRN_CONFIG` selects it for the CLI, stdio MCP, setup, and hooks.
Otherwise the file is `$XDG_CONFIG_HOME/cairn/config.json`, else
`~/.config/cairn/config.json`, else `%APPDATA%/cairn/config.json`. A 0.1.4 file
with those two fields still loads. If `CAIRN_URL` or `CAIRN_DEVICE_TOKEN` is
present, including an empty value, it is ignored and one stderr line names the
variable, not the value. An HTTP 401 names the selected file and says to run
`cairn setup <server-url>` with `CAIRN_CONFIG` set to that file.

Refresh owned integrations with
`cairn setup <server-url> --agent claude|codex|opencode`. That installs the
hook and instruction files embedded in this binary and rewrites only the
Cairn-owned hooks, MCP entry, and OpenCode `plugins/cairn/index.js`. URL-only
setup still downloads the hook scripts of the matching published release.
A script that differs from the recorded checksum is kept as `<name>.bak`.

`memory_search` returns short cards (`id`, `kind`, `title`, `summary`, `score`,
and `status` for a stale memory). `memory_save` returns `id`, `status` and
`deduplicated`. `memory_get` returns full records. A relevance floor can return
no hits. Results are one JSON text channel. There is no server LLM.

OpenCode 2.1.4 omits the MCP registry on the first prompt of a cold session.
The second prompt in that warmed session advertises the Cairn tools. The
2026-11-18 check did not treat a newer OpenCode build as the fix. Claude Code
and Codex native probes in that pass did not emit a compaction event; their
compaction check remains the fixture `SessionStart` with `source` `compact`.
An earlier OpenCode 2.1.4 loopback session did compact and re-inject the brief.
The record is [v0.1.5 release verification](evidence.md).

Rollback to 0.1.4 does not rewrite server rows or roll the SQL schema back.
The 0.1.4 init and rename behavior in the section above stays in force on that
version. `cairn update` will not install an older version. Stop the agents
that run Cairn hooks first. While the 0.1.5 CLI is still the one on `PATH`,
remove each owned integration with
`cairn setup <server-url> --agent claude|codex|opencode --remove`
(`--agent` immediately after the URL, and `--remove` last). That command
reads no token. The 0.1.4 hook accepts only `--agent claude|codex`, and 0.1.4
setup does not delete the managed OpenCode file `plugins/cairn/index.js`.

The 0.1.5 hook keeps `hook-state.sqlite3` beside the selected config file.
Its `sessions` table has five columns, including `generation`, and inserts with
`VALUES (?, ?, ?, ?, ?)`. The 0.1.4 hook uses that same file and
`INSERT OR REPLACE INTO sessions VALUES (?, ?, ?, ?)`, and `CREATE TABLE IF
NOT EXISTS` leaves the newer table in place, so the restored hook fails on it.
Move `hook-state.sqlite3` aside, with any `-journal`, `-wal`, or `-shm`
sidecar present, or restore a 0.1.4 backup of that file. Leave the credential
file and any outbox queue untouched.

Then replace the matched 0.1.4 server, CLI, and hook scripts by hand, and
reinstall the integrations you still want with that binary. The restored
client makes `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` together replace the config
file, drops `CAIRN_CONFIG`, and brings back the previous MCP payloads, default
capture visibility, relevance behavior, and Stop save-ask.

## 0.1.6 upgrade notes

0.1.6 replaces the storage and the memory model; see the [0.1.6 foundation specification](specs/0.1.6-foundation.md).
There is no migration from 0.1.5. The server uses PostgreSQL only, with five migrations (`migrations/0001` to `0005`)
in the schema `cairn`. A database that still has the old `public.cairn_memories` table is refused at startup with
`pre-0.1.6 database; recreate it`; the server never drops data itself. The file store, `--store`, the embedded
`/viewer`, `/import`, `/sync/pull` and the similarity-review routes and tools are gone. Upgrade the server and the
CLI together: the MCP tool set and the shape of `memory_save` changed.

The agent hook is now the `cairn hook` command and the Python scripts are gone. `POST /sync/push` and `/context` are removed, so a machine that still runs the Python hook cannot capture until it is updated. On every machine: install the new `cairn`, then re-run `cairn setup <server-url> --agent claude|codex|opencode` (the token comes on stdin). That replaces the old `python3 cairn_hook.py` entries, deletes the old `hook/` scripts and `hook-state.sqlite3`, and leaves an old version 2 outbox untouched. `cairn update` also rewrites the commands of agents that already have them.

The server URL given to `cairn setup` may end in `/cairn-memory`; the client drops that suffix once, so `https://host`, `https://host/cairn-memory` and a reverse-proxy prefix such as `https://host/agent` all work.

The release also carries three stub assets, `cairn_hook.py`, `cairn_outbox.py` and `cairn_instructions.txt`
(`release/legacy-update-stubs`). An installed 0.1.5 `cairn update` downloads and checksums them before it swaps
the binary and aborts when one is missing, so they must stay in `release.yml` until 0.1.5 clients are gone;
`scripts/check_update_assets.py` (the Documentation validation job) fails when a name the 0.1.5 client asks for is
no longer published. 0.1.5 records their checksums in `hook/installed.json`, so the first `cairn setup` of 0.1.6
deletes them (`retire_python`).

## 0.1.7 upgrade notes

0.1.7 changes no schema and no tool; the details are in [the 0.1.7 release notes](release-notes-0.1.7.md). Upgrade the
server and the CLI together, start the new server on the existing database (no wipe), and run `cairn update` on each
machine. Re-running `cairn setup` is needed only when `cairn doctor` warns; when it runs, it also deletes a leftover
0.1.5 `hook/run-hook` (only if it calls `cairn_hook.py`) and `hook/README.txt` with it. Search scores changed meaning
(absolute, capped at 1.0), so a client that compared them with a fixed threshold must be re-checked.
Memories saved by 0.1.6 keep the old identity hash (it included the reason), so saving the same text once more makes a
duplicate that consolidation merges; there is no backfill.

## 0.1.8 upgrade notes

0.1.8 changes no schema and no tool; the details are in [the 0.1.8 release notes](release-notes-0.1.8.md). Upgrade the
server and the CLI together, start the new server on the existing database (no wipe), and run `cairn update` on each
machine. Entity extraction keeps the leading dot of a bare dot-directory path (`.cairn/project.json`), and search cards,
`memory_get` records and the `memory_feedback` reply carry `stale_cause`. Memories saved earlier keep their stored
entities (no backfill): one the old extractor marked `missing` stays stale until it is saved again with `supersedes`.

## How clients update

- `cairn --version` prints the installed version.
- `cairn update --check` reports whether a newer release exists. `cairn update` downloads the binary for the compile-time target, verifies it against the release's `SHA256SUMS`, stages the binary next to the running one with the same permission bits, runs `<staged> --version` (2 second limit) and requires it to print `cairn <new version>`, and only then replaces the running executable (an atomic rename on unix; on Windows the old file is renamed to `cairn.exe.old`, or `cairn.exe.old-<time>` if that name is stuck, and removed on a later run). Any failure leaves the installed `cairn` untouched; an unwritable directory fails with a message to re-run with permission to write there.
- The hook is the `cairn` binary itself (`cairn hook <Event>`), so there are no scripts to match. `cairn setup --agent` writes the hook commands with the absolute path of the running executable, and after replacing the binary `cairn update` rewrites those commands for every agent that already has a Cairn hook, so they follow the executable.
- Interactive commands print one line to stderr when a newer release is known, for example `cairn 0.2.0 is available (you have 0.1.0) — run `cairn update``. The release check runs at most once every 24 hours (2 second timeout, failures silent, result cached in `update-check.json` beside the config). It never runs for `cairn mcp`, `cairn update`, when stderr is not a terminal, or when `CAIRN_NO_UPDATE_CHECK=1`. The hook and the MCP server never check or update.
- `CAIRN_UPDATE_URL` replaces the GitHub latest-release API URL (it must end in `/latest`) in debug builds only, for tests; a release build ignores it. Update URLs must be https; plain http to loopback is accepted in debug builds only.
- `cairn-server` is not self-updating: operators download it from the release and follow the migration notes ([ADR 0004](./adr/0004-trunk-based-development-and-release.md)).
