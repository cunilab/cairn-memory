---
id: CM-CONTRACT-005
title: "Layer 1 Hook Capture Adapter"
status: draft
---

# Layer 1 Hook Capture Adapter

`scripts/cairn_hook.py` is the portable hook boundary for local integrations.
It lets an agent hook send one JSON object without knowing Cairn's internal
storage or lifecycle rules.

## Invocation

```bash
printf '%s\n' '{"sessionId":"session-1","content":"The test passed.","role":"tool","source":"PostToolUse"}' \
  | CAIRN_URL=http://127.0.0.1:3111 python3 scripts/cairn_hook.py
```

`CAIRN_URL` defaults to `http://127.0.0.1:3111`. If `eventId` is absent, the
adapter derives a stable fingerprint from the session, source, and content so
retries remain idempotent. If `sessionId` is absent, `CAIRN_SESSION_ID` is
required.

## Normalized fields

The adapter accepts top-level or nested `payload` values for:

- `eventId` / `event_id`
- `sessionId` / `session_id`
- `content`, `text`, or `message`
- `role`
- `source` or `type`
- `importance`
- `context`
- `lifecycle`

The normalized request is sent to `POST /cairn-memory/observe`. Cairn performs
redaction, lifecycle classification, retention, persistence, and validation.
The adapter never prints the input payload on failure, reducing accidental
secret leakage in hook logs.
