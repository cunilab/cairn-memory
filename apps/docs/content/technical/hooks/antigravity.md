---
id: CM-DOC-TECHNICAL-HOOKS-ANTIGRAVITY
title: "Antigravity CLI hooks"
status: accepted
description: "PreInvocation input, deferred context and the available lifecycle events."
---

# Antigravity CLI hooks

Lifecycle hook integration and deferred context injection for Google's Antigravity CLI.

- **`SessionStart`**: Evaluates workspace context and injects the opening brief.
- **`PreInvocation`**: Extracts latest user prompt, deduplicates model invocations within a turn, and injects prompt recall via `injectSteps`.
- **`PreToolUse`**: For `run_command`, returns `allow` with a warning reason on matching commands.
- **`PostToolUse`**: Captures tool results; queues failed-command cards for system injection in the subsequent `PreInvocation`.
- **`Stop`**: Checkpoints session work.
- **Limits**: Compact and session-end events are not available in Antigravity.

See [Host delivery matrix](index.md#command-recall-by-host) and [Antigravity CLI setup](../../users/agents/antigravity.md).
