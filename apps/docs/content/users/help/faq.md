---
id: CM-DOC-USERS-HELP-FAQ
title: "FAQ"
status: accepted
description: "Answers about memory visibility, local models, upgrades and host memory."
---

# FAQ

## Who can read a memory?

Personal memory belongs to its owner, project memory to the project's members, and global memory to authenticated accounts on the same server. A server account with the project ID can join a project. Hermes chat-tagged personal memory has an additional chat boundary and remains visible in the gateway account's console. [Scopes](../memory/scopes.md) · [Chat visibility](../agents/hermes-gateway.md#hermes-chat-recall)

## Does Cairn need a server LLM?

Cairn's background distillation is rule-based. When the local MiniLM model is available, it adds embeddings to search. Lexical, entity and link retrieval continue with semantic search disabled. The server does not call an external model API. [Retrieval](../../technical/retrieval.md#hybrid-recall)

## What survives upgrades?

The PostgreSQL store holds the durable records. Keep its data volume and back up the database before upgrading. The server applies migrations at startup, and the notes for your version describe exceptions such as the fresh-store boundary at 0.1.6. [Upgrade steps](../upgrade.md) · [Backup and restore](../../technical/backups.md)

## What happens to the host's native memory?

Claude Code's optional import keeps the original note files and backs up the old index before writing its Cairn pointer. Use `--keep-native-memory` to keep native writes enabled. Hermes's built-in notes and enablement settings remain in its profile. Its external provider selection changes to Cairn and can be restored by removal. [Claude Code](../agents/claude-code.md#native-memory) · [Hermes](../agents/hermes.md#connect-a-profile)

## Can I use Cairn without Git?

Yes. Use `cairn init` and keep `.cairn/project.json` in the project folder. The CLI and MCP resolve it from the nearest containing folder. [Membership](../memory/scopes.md#init-and-membership)
