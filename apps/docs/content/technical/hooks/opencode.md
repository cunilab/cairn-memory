---
id: CM-DOC-TECHNICAL-HOOKS-OPENCODE
title: "OpenCode V2 hooks"
status: accepted
description: "OpenCode plugin events and deferred delivery in model context."
---

# OpenCode V2 hooks

The native V2 plugin uses `ctx.tool.hook("execute.before")` to look up command warnings. It queues the result because the callback returns void. `ctx.tool.hook("execute.after")` receives completed and failed results and can queue related memories for failures. A prompt hook similarly queues prompt matches. The next `ctx.session.hook("context")` delivers the pending context.

The plugin checkpoints idle work at Stop. It cannot continue the turn with a Stop reminder. [Host delivery matrix](index.md#command-recall-by-host) · [OpenCode setup](../../users/agents/opencode.md).
