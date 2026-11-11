---
id: CM-CONTRACT-003
title: "Cairn Export Format v1"
status: draft
---

# Cairn Export Format v1

The `cairn-memory` export is a portable JSON bundle for local backup, restore,
and migration. It contains durable state and forget tombstones, but never
provider credentials or unredacted captured secrets.

## Envelope

```json
{
  "format": "cairn-memory",
  "version": 1,
  "exportedAt": "1771977600",
  "sessions": [],
  "observations": [],
  "memories": [],
  "forgottenIds": []
}
```

The session, observation, and memory entries use the serialized domain types.
Memory and recall-related entries may include the additive Layer 2 `scope`
field (`session`, `worktree`, `branch`, `project`, or `global`). Memory entries
also include lifecycle, `kind`, `next_steps`, `unresolved_work`, and optional
`supersedes` fields.
`forgottenIds` is the authoritative suppression set for imported state.

## Import behavior

- The format and version are validated before any state is changed.
- IDs are the merge keys: session ID, event ID, and memory ID respectively.
- Re-importing an identical bundle is a no-op and returns zero imported items.
- A same-ID, different-content conflict fails the complete import rather than
  silently replacing local state.
- Imported observation and memory content is redacted and bounded before it is
  persisted.
- Forget tombstones are merged and remain effective after restart.
- Import does not call providers or replay capture hooks.

## Interfaces

- CLI: `cairn export <store> <output>` and `cairn import <store> <input>`
- REST: `POST /cairn-memory/export` and `POST /cairn-memory/import`
- MCP: `memory_export` and `memory_import`

This remains a Layer 1/2 local format. Identity, authorization, encryption,
promotion, and cross-device conflict policy are intentionally later concerns.
