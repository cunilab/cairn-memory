---
id: CM-REL-001
title: "Cairn Memory Release Process"
status: draft
description: "Cut a release from main, publish binaries and images, and update installed clients."
---

# Cairn Memory Release Process

Trunk-based development, automated CI gates, GitHub multi-platform release assets, and client updates.

## Trunk-based flow

- **Trunk stability**: `main` is always releasable. Direct pushes are disabled.
- **Branches**: Short-lived branches (`feat/`, `fix/`, `docs/`, `chore/`) merge via pull request after passing all checks.
- **Required CI gates**:
  1. Rust formatting and clippy (`cairn-core` has zero async or network dependencies)
  2. Workspace tests against PostgreSQL 17 + pgvector
  3. Native builds on macOS and Windows runners
  4. Web console typecheck, lint, and build
  5. Documentation validation (`validate_phase0.py`)
  6. Acceptance smokes (`scripts/smoke_mcp.sh` and `scripts/smoke_web.sh`)
  7. Multi-arch Docker container builds
  8. Benchmark evaluation (`bench.py` v1 and v2 gates)

## Cut a release

Version numbers follow semantic versioning (`vMAJOR.MINOR.PATCH`) defined in the root `Cargo.toml`.

1. **Version bump**: Update `[workspace.package].version` in `Cargo.toml`, sync `apps/web/package.json` and `package-lock.json`, and run `cargo check`.
2. **Release notes**: Add `apps/docs/content/users/releases/<version>.md` and update the [release notes index](../users/releases/index.md).
3. **Merge and tag**: Merge the PR into `main` and tag the release commit:
   ```sh
   git switch main && git pull --ff-only
   git tag v0.2.0
   git push origin v0.2.0
   ```
4. **Automated release workflow**:
   - Builds native CLI and server binaries for macOS (`aarch64`), Linux (`x86_64`, `aarch64`), and Windows (`x86_64`).
   - Publishes GitHub Release assets and `SHA256SUMS`.
   - Builds and publishes multi-arch container images to GitHub Container Registry (`ghcr.io/cunilab/cairn-memory-server` and `cairn-memory-web`).

## Publish the documentation

The GitHub Pages workflow automatically builds the tagged documentation site at `https://cunilab.github.io/cairn-memory/` upon push of a stable release tag.

## Decisions and why

- Single-source versioning ensures alignment between Rust crates, the web console, and container tags.
- Client updaters verify release checksums against staged temporary files before replacing active binaries.

## How clients update

Installed `cairn` CLI clients check the latest GitHub Release via `cairn update --check` and apply updates via `cairn update`. The client downloads the platform binary, validates its SHA-256 hash, stages it in a temporary file, and replaces the executable.
