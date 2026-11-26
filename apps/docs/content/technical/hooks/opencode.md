---
id: CM-DOC-TECHNICAL-HOOKS-OPENCODE
title: "OpenCode V2 hooks"
status: accepted
description: "OpenCode plugin events and deferred delivery in model context."
---

# OpenCode V2 hooks

OpenCode V2 server plugin hooks, execution interception, and deferred context delivery.

The native OpenCode V2 plugin integrates with server lifecycle hooks:

- **`execute.before`**: Looks up command warnings prior to shell execution. Queues results because the hook return is void.
- **`execute.after`**: Captures execution outcomes; queues related gotchas and error remedies when commands fail.
- **Prompt hook**: Analyzes user prompts and queues matching preference or project cards.
- **`session.hook("context")`**: Injects queued warnings, prompt cards, and error context into the subsequent model context.
- **`Stop`**: Checkpoints active work. OpenCode does not support continuing turns from a Stop reminder.

See [Host delivery matrix](index.md#command-recall-by-host) and [OpenCode setup](../../users/agents/opencode.md).
