---
id: CM-DOC-USERS-RELEASES-INDEX
title: "Release notes"
status: accepted
description: "Compatibility, migrations and upgrade notes for each version."
---

# Release notes

Version history, breaking changes, database migrations, and upgrade instructions for Cairn Memory releases.

| Version | Key changes | Upgrade check |
| --- | --- | --- |
| [0.1.11](0.1.11.md) | Hermes memory provider, profile cleanup on removal, Dependabot resolution | Requires Hermes ≥ v2026.4.23; refresh each active profile. |
| [0.1.10](0.1.10.md) | Layered recall (L0–L3), memory trust, pins, global skill library, Claude import | 11 startup migrations; refresh client hooks and instructions. |
| [0.1.9](0.1.9.md) | Project handoffs, non-Git init, Stop reminder notes, automated Fixes, aliases | 9 startup migrations; run `cairn setup <agent>` after update. |
| [0.1.8](0.1.8.md) | Stale cause explanations (`feedback` vs `missing`), dot-directory path fix | No migrations; client and server update together. |
| [0.1.7](0.1.7.md) | Absolute ranking scores, duplicate deduplication, cleaner handoffs | No migrations; client and server update together. |
| [0.1.6](0.1.6.md) | Rust hooks, 5 MCP tools, local MiniLM embeddings, new web console | **Fresh database required**; breaking schema change from 0.1.5. |

[Upgrade guide](../upgrade.md) · [GitHub release assets](https://github.com/cunilab/cairn-memory/releases)
