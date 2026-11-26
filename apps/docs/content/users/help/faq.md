---
id: CM-DOC-USERS-HELP-FAQ
title: "FAQ"
status: accepted
description: "Answers about memory visibility, local models, upgrades and host memory."
---

# FAQ

Frequently asked questions regarding memory visibility, model requirements, upgrades, and Git.

## Who can read a memory?

- **Personal**: Only the account that created the record.
- **Project**: Accounts registered as members of that project via `.cairn/project.json`.
- **Global**: All authenticated users on the server (e.g. shared skills).
- **Hermes Gateway**: Personal memories created in chats carry a chat thread tag and are visible only within that thread and the owner's web session.
- See [Projects and memory scopes](../memory/scopes.md) and [Hermes chat recall](../agents/hermes-gateway.md#hermes-chat-recall).

## Does Cairn need a server LLM?

No. Cairn's background distillation and quality checks are rule-based and run locally. When configured, an embedded local MiniLM model computes 384-dimensional vectors for semantic search. If disabled, lexical BM25, entity matching, and link graph traversal operate without external LLM dependencies. See [Hybrid recall](../../technical/retrieval.md#hybrid-recall).

## What survives upgrades?

All durable state lives in PostgreSQL (accounts, credentials, projects, memories, links, and jobs). Back up your database before updating. Database migrations run automatically at server startup. See [Upgrade Cairn](../upgrade.md) and [Backup and restore](../../technical/backups.md).

## What happens to the host's native memory?

- **Claude Code**: The optional import backs up `MEMORY.md` to `MEMORY.md.pre-cairn` and writes a pointer to Cairn. Native note files remain on disk. Pass `--keep-native-memory` to leave native file writes enabled.
- **Hermes Agent**: Built-in memory files (`MEMORY.md`, `USER.md`) remain untouched. Cairn configures itself as the external memory provider and can be reverted cleanly via `cairn setup hermes --remove`.
- See [Claude Code native memory](../agents/claude-code.md#native-memory) and [Hermes profile setup](../agents/hermes.md#connect-a-profile).

## Can I use Cairn without Git?

Yes. Run `cairn init` in any directory to generate `.cairn/project.json`. The CLI and MCP tools resolve project context from the nearest parent directory containing that file. See [Membership](../memory/scopes.md#init-and-membership).
