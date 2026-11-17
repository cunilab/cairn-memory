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
  - `Rust format and clippy`
  - `Rust tests`
  - `PostgreSQL tests` (each ignored test runs against its own fresh PostgreSQL 17 database with pgvector, see `scripts/ci_postgres_tests.sh`)
  - `Build on macos-15` and `Build on windows-latest`
  - `Web console typecheck, lint, build`
  - `Python hook and outbox tests`
  - `Documentation validation`
- Renaming a job in `ci.yml` detaches it from the branch protection rule; update the rule in the same change.
- The workflows do not run `scripts/smoke_full_flow.sh` or `scripts/smoke_layer3.sh`; run them locally for transport, persistence, or sync changes (see [AGENTS.md](../AGENTS.md)).

## Cut a release

Releases are tags `vMAJOR.MINOR.PATCH` on `main`. The version lives once, in `[workspace.package]` of the root `Cargo.toml`.

1. On a branch (for example `chore/release-0.2.0`), change `version` in `[workspace.package]`, set `VERSION` in `scripts/cairn_outbox.py` to the same value (it forms the hook's `cairn-hook/<version>` User-Agent; `scripts/smoke_outbox.py` fails when they differ), and let `Cargo.lock` follow (`cargo check`). Put migration notes, upgrade guidance, and compatibility or rollback notes for operators in the pull request description (ADR 0004 requires them with a release).
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
   - creates the GitHub Release with generated notes prefixed by server runtime/upgrade requirements and these assets: `cairn-<target>[.exe]`, `cairn-server-<target>[.exe]`, `cairn_hook.py`, `cairn_outbox.py`, and `SHA256SUMS` covering every other asset. Binaries are raw files, not archives;
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

## How clients update

- `cairn --version` prints the installed version.
- `cairn update --check` reports whether a newer release exists. `cairn update` downloads the binary for the compile-time target and the hook scripts, verifies each against the release's `SHA256SUMS`, stages the binary next to the running one with the same permission bits, runs `<staged> --version` (2 second limit) and requires it to print `cairn <new version>`, and only then replaces the running executable (an atomic rename on unix; on Windows the old file is renamed to `cairn.exe.old`, or `cairn.exe.old-<time>` if that name is stuck, and removed on a later run). Any failure leaves the installed `cairn` untouched; an unwritable directory fails with a message to re-run with permission to write there.
- `cairn setup` installs the hook scripts of the release for the running binary's version (`v<version>`), and `cairn update` installs those of the release it installs, so scripts and binary always match. They go in `hook/` next to the config file (`~/.config/cairn/hook/` by default, mode 0700), and `hook/installed.json` records their checksums. A script that differs from the recorded one was edited locally: its content is kept as `<name>.bak` and the command prints a note. A build with no matching release (a development build) skips the hook scripts.
- Interactive commands print one line to stderr when a newer release is known, for example `cairn 0.2.0 is available (you have 0.1.0) — run `cairn update``. The release check runs at most once every 24 hours (2 second timeout, failures silent, result cached in `update-check.json` beside the config). It never runs for `cairn mcp`, `cairn update`, when stderr is not a terminal, or when `CAIRN_NO_UPDATE_CHECK=1`. The hook and the MCP server never check or update.
- `CAIRN_UPDATE_URL` replaces the GitHub latest-release API URL (it must end in `/latest`) in debug builds only, for tests; a release build ignores it. Update URLs must be https; plain http to loopback is accepted in debug builds only.
- `cairn-server` is not self-updating: operators download it from the release and follow the migration notes ([ADR 0004](./adr/0004-trunk-based-development-and-release.md)).
