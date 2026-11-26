---
id: CM-DOC-USERS-AGENTS-ANTIGRAVITY
title: "Antigravity CLI"
status: accepted
description: "Connect Antigravity CLI to Cairn and verify its managed integration."
---

# Antigravity CLI

```sh
cairn setup https://your-server.example --agent antigravity
cairn doctor
agy mcp list
```

Supply the device token on stdin. On a configured machine use `cairn setup antigravity`. Restart the CLI. The managed group registers SessionStart, PreInvocation, PreToolUse, PostToolUse and Stop. Setup grants `mcp(cairn/*)` in `~/.gemini/antigravity-cli/settings.json`. Explicit ask and deny rules still apply. [Antigravity CLI hook details](../../technical/hooks/antigravity.md).
