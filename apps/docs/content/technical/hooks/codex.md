---
id: CM-DOC-TECHNICAL-HOOKS-CODEX
title: "Codex hooks"
status: accepted
description: "Codex event mapping, hook results and command failure detection."
---

# Codex hooks

Lifecycle event integration, advisory context responses, and error detection in OpenAI Codex CLI.

- **`SessionStart` & `UserPromptSubmit`**: Return `hookSpecificOutput.additionalContext` with session brief and prompt recall.
- **`PreToolUse`**: Bash execution checks return advisory command warnings in `additionalContext`.
- **`PostToolUse`**: Captures tool results; attaches advisory gotchas when Codex reports command failure.
- **`Stop`**: Checkpoints session work; can return `decision: "block"` to present the Stop save reminder.
- **Unified exec**: When unified exec returns unformatted output without an explicit exit status, Cairn remains silent to avoid false error recall.

See [Supported hook events](index.md#events) and [Codex setup](../../users/agents/codex.md).
