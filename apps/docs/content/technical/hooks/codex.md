---
id: CM-DOC-TECHNICAL-HOOKS-CODEX
title: "Codex hooks"
status: accepted
description: "Codex event mapping, hook results and command failure detection."
---

# Codex hooks

`SessionStart` and `UserPromptSubmit` can return `hookSpecificOutput.additionalContext`. Shell `PreToolUse` can return the same envelope for a command warning. `PostToolUse` captures tool activity and can return advisory context when Codex reports a failure. Stop checkpoints the session and can return `decision: "block"` with the once-per-session reminder as its reason.

Unified exec can return plain output with no exit status. Cairn only considers failed-command recall when it can identify a failure from the host output. Unknown status produces no failed-command advisory.

[Supported hook events](index.md#events) · [Install the Codex integration](../../users/agents/codex.md).
