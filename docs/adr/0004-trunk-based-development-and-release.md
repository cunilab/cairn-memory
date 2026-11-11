---
id: CM-ADR-0004
title: "ADR-0004: Use trunk-based development with isolated worktrees and GitHub releases"
status: draft
---

# ADR-0004: Use trunk-based development with isolated worktrees and GitHub releases

- **Date:** 2026-10-25
- **Milestone:** Phase 0 — product, contract, and implementation baseline
- **Related:** [Roadmap](../roadmap.md), [Documentation standards](../standards.md)

## Context

Cairn Memory may be changed by multiple people and coding agents at the same time. Shared working directories create accidental file collisions, unclear ownership, and difficult review. Long-lived branches also increase merge drift and make it harder to keep the documentation, contracts, and implementation aligned.

The project needs a simple release model with reproducible version numbers, protected integration, and a clear relationship between code, migrations, documentation, and server upgrades.

## Decision

1. Use `main` as the protected trunk branch.
2. Use short-lived branches for each issue, feature, bug fix, documentation change, or migration.
3. Use one Git worktree per active branch/person/agent when concurrent work is needed. No two workers share a mutable working directory.
4. Branch names use a small convention such as `feat/CM-123-short-name`, `fix/CM-123-short-name`, `docs/CM-123-short-name`, or `chore/CM-123-short-name`.
5. Every branch must rebase or merge from current `main` before integration according to the repository policy; integration happens through a pull request.
6. Pull requests require the relevant tests, documentation checks, database migration checks, and review of affected ADR/spec documents.
7. Releases are versioned with semantic version tags such as `v0.1.0`, created from `main`, and published as GitHub Releases.
8. A release must include server version, database migration notes, upgrade guidance, compatibility notes, and rollback guidance.
9. The web administration UI exposes the running version, migration state, and available release guidance; upgrades remain a server/operator action rather than an unreviewed browser-side mutation.
10. Worktree metadata and branch identity are captured in agent sessions when available: repository, branch, worktree, commit SHA, base branch, and pull request.

## Consequences

- Concurrent agents and people can work safely without sharing uncommitted files.
- Branch-aware memory can distinguish local or unmerged decisions from project-wide facts.
- GitHub Releases provide a clear upgrade boundary for the server.
- Short-lived branches reduce integration drift.
- The project needs branch protection, CI, PR checks, release automation, and worktree cleanup discipline.

## Alternatives considered

### GitFlow with long-lived develop and release branches

Rejected because it creates more branch state and merge ceremony than this project needs while the product is evolving quickly.

### One shared worktree for all agents

Rejected because concurrent edits and generated files become difficult to attribute and review safely.

### Browser-triggered automatic production upgrade

Rejected because database migrations, backups, rollback, and process restarts need explicit operator control.
