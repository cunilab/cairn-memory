---
id: CM-GUIDE-003
title: "Cairn Phase 0 Fixtures"
status: accepted
---

# Cairn Phase 0 Fixtures

These fixtures are small, redacted contract fixtures for Phase 0. They are intended to exercise the memory policy and retrieval expectations before the Rust runtime exists. They are not yet the final wire format.

## Fixture schema

Each JSONL record uses this envelope:

```json
{
  "schemaVersion": "cairn.fixture/v0.1",
  "eventId": "evt-...",
  "type": "observation | session.start | session.end | command | test | memory.forget | device.revoke | sync.replay",
  "actor": "person | agent | device | service",
  "space": "session | branch | personal | project",
  "sessionId": "...",
  "context": {},
  "payload": {},
  "expected": {}
}
```

The `expected` object describes the behavior a future implementation must prove. It is deliberately explicit about scope, redaction, retrieval, and visibility. Fixture data contains no real credentials or private payloads.

## Scenarios

| Fixture | Exercises |
| --- | --- |
| `session-continuity.jsonl` | Automatic capture, a useful decision, test evidence, session summary, and next-session recall |
| `branch-context.jsonl` | Branch/worktree scope, project promotion eligibility, and non-leakage into another branch |
| `sensitivity.jsonl` | Secret redaction, private retention, provider isolation, and no automatic sharing |
| `deletion-revocation-offline.jsonl` | Forget, device revocation, offline replay, duplicate delivery, and purge acknowledgement |
| `golden-corpus/v0.1/cases.json` | Versioned baseline questions, expected evidence, forbidden results, and scoring fields |

## Rules

- Fixtures must remain deterministic and readable by humans.
- Never add real API keys, tokens, passwords, private source code, or user transcripts.
- New fields require a schema-version change or an explicit backward-compatible note.
- A future contract test should consume these fixtures without changing their meaning.
