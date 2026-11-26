---
id: CM-DOC-TECHNICAL-HOOKS-GROK
title: "Grok Build hooks"
status: accepted
description: "Grok Build delivers the brief and prompt suggestions with tool results."
---

# Grok Build hooks

SessionStart records the checkout baseline. Grok ignores its advisory output, so the first PostToolUse result delivers the brief. UserPromptSubmit captures and looks up the prompt, then defers matching cards to the next PostToolUse.

A Bash PreToolUse lookup runs before execution and Grok presents its advisory with the tool batch result. PostToolUse can report nonzero exit status and carry related Cairn memories as `additionalContext`. Grok also supports Stop, PreCompact and SessionEnd.

[Host delivery matrix](index.md#command-recall-by-host) · [Grok Build setup](../../users/agents/grok.md).
