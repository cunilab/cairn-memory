---
id: CM-DOC-USERS-HELP-TROUBLESHOOTING
title: "Troubleshooting"
status: accepted
description: "Resolve connectivity, recall, project and capture problems from their symptoms."
---

# Troubleshooting

Diagnose and resolve connection errors, missing memories, semantic search misses, and capture delays.

## Common symptoms and solutions

| Symptom | Diagnostic check | Recommended resolution |
| --- | --- | --- |
| **Cannot connect to server** | Run `cairn doctor` | Verify URL in config matches server. Ensure HTTPS for remote addresses (HTTP allowed only on loopback). Include `/agent` prefix if using reverse proxy routing ([Deployment](../../technical/deployment.md#routing)). |
| **Server returns HTTP 401** | Check device status in console | Device token is revoked or missing. Generate a new token under **Settings → Devices** and run `cairn setup <url>`. |
| **Expected project memory missing** | Verify `.cairn/project.json` | Confirm your account is an active member. Check whether the project was archived in the console ([Archive and restore](../memory/scopes.md#archive-and-restore)). |
| **Exact search works, paraphrases miss** | Check semantic search health | Verify MiniLM model files on server. Paraphrase recall relies on cosine similarity; exact lexical and entity matching remain operational. |
| **Capture appears delayed** | Inspect `.cairnignore` and stderr | Check if `.cairnignore` exists in checkout root. Temporary network drops buffer events in the client [outbox](../../technical/hooks/index.md#outbox) for automatic replay. |
| **Automatic recall quiet** | Review recall quotas | Check if memory was muted after 3 uncredited pushes ([Recall layers](../memory/layers.md#automatic-recall)). Query explicitly via `memory_search` to restore eligibility. |
| **Background jobs failing** | Review **Activity → Jobs** | Inspect quarantined jobs in the web console as administrator. Workers retry with exponential backoff. |
| **Hermes chat memory disabled** | Verify profile and versions | Requires Hermes ≥ v2026.4.23 and server ≥ 0.1.10. Restart gateway and issue `/new` to reload provider schemas ([Hermes gateway](../agents/hermes-gateway.md)). |
