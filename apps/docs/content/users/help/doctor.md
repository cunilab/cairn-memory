---
id: CM-DOC-USERS-HELP-DOCTOR
title: "Doctor"
status: accepted
description: "Read configuration, connection, device and integration checks and apply device fixes."
---

# Doctor

Run system diagnostics to verify credentials, database connectivity, and agent configurations.

```sh
cairn doctor
```

Outputs line-by-line checks prefixed with `ok`, `warn`, or `fail`. Exits with code `1` if any check fails.

## Health checks

| Subsystem | Verified items | Resolution if failing |
| --- | --- | --- |
| **Config, server, device** | Local config file readability, server reachability, and valid device token | Re-run `cairn setup <url>` with a fresh token from **Settings → Devices**. |
| **Semantic search** | Local MiniLM model directory and ONNX runtime health | Inspect model volume and server logs. Lexical and entity recall remain functional while semantic search is offline. |
| **Project** | Presence and syntax of `.cairn/project.json` | Run `cairn init` in the checkout root to register or attach the repository. |
| **Agent integrations** | Managed instruction blocks and hook configuration files | Re-run `cairn setup <agent>` to repair missing or outdated instruction markers. |
| **Claude native memory** | Integrity of `MEMORY.md` pointer file | Run `cairn import claude-memory` in the project root to restore the Cairn pointer. |
| **Hermes** | Host version (≥ v2026.4.23) and memory provider setting | Upgrade Hermes Agent and verify `memory.provider: cairn-memory` in `config.yaml`. |

## Automated fixes

When doctor reports available fixes, inspect and apply them:

```sh
# List pending fixes
cairn fixes
# Apply a specific fix
cairn fixes apply <id>
```

See [CLI reference](../reference/cli.md).
