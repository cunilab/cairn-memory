---
id: CM-DOC-USERS-HELP-DOCTOR
title: "Doctor"
status: accepted
description: "Read configuration, connection, device and integration checks and apply device fixes."
---

# Doctor

```sh
cairn doctor
```

Each line starts with `ok`, `warn` or `fail`, followed by the check and detail. Any failure makes the command exit 1. If the configured device has a pending device fix, doctor can apply it.

| Check | What to do if it reports a problem |
| --- | --- |
| Config, server or device | Read the named config path and confirm the server address and the device status in Settings. Run setup again with a current token if the server rejects it. |
| Semantic search | Check deployment health and the model directory. Lexical, entity and link recall can operate while semantic search is disabled. |
| Project | Inspect `.cairn/project.json` and run `cairn init` in the checkout when you intend to attach it. |
| Integration or instructions | Run `cairn setup <agent>` with the intended host profile. Repair malformed instruction markers before refreshing. |
| Claude native memory | Run [Claude memory import](../agents/claude-code.md#import-claude-code-memory) in the project if setup manages native memory and the pointer is missing. |
| Hermes | Check the [minimum host version and profile](../agents/hermes.md). Doctor checks provider selection for Hermes. |

Run `cairn fixes` to see device fixes or `cairn fixes apply [id]` to run them directly. [CLI reference](../reference/cli.md).
