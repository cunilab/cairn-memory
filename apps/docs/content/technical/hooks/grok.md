---
id: CM-DOC-TECHNICAL-HOOKS-GROK
title: "Grok Build hooks"
status: accepted
description: "Grok Build delivers the brief and prompt suggestions with tool results."
---

# Grok Build hooks

Event processing and deferred context delivery mechanisms for Grok Build CLI.

- **`SessionStart`**: Captures initial workspace state. Because Grok discards advisory output on startup, the brief is deferred and delivered with the first `PostToolUse` tool result.
- **`UserPromptSubmit`**: Captures and analyzes prompt; defers matched cards to the next `PostToolUse`.
- **`PreToolUse`**: Evaluates command warnings and attaches advisory notices to the tool batch. Grok delivers this context together with the tool result, not before the command runs.
- **`PostToolUse`**: Captures tool execution; returns related gotchas and remedies when commands exit with non-zero status.
- **Checkpoints**: Supports `Stop`, `PreCompact`, and `SessionEnd` for state distillation.

See [Host delivery matrix](index.md#command-recall-by-host) and [Grok Build setup](../../users/agents/grok.md).
