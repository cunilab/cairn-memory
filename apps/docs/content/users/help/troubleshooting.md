---
id: CM-DOC-USERS-HELP-TROUBLESHOOTING
title: "Troubleshooting"
status: accepted
description: "Resolve connectivity, recall, project and capture problems from their symptoms."
---

# Troubleshooting

| Symptom | Check |
| --- | --- |
| Setup or a tool cannot connect | Run [doctor](doctor.md). Compare the server address with the setup command under Settings. Public addresses need HTTPS. Include `/agent` if your [proxy](../../technical/deployment.md#routing) expects it. |
| The server reports 401 | Check that your device is active and rerun setup with a device token from Settings. The selected config file supplies the credential. |
| Expected project memory is missing | Confirm membership and the project ID in the current checkout. [Archived projects](../memory/scopes.md#archive-and-restore) stay visible in the console and are hidden from agent retrieval. |
| Search works for exact words, but misses paraphrases | Use distinct terms or exact entities, then check `semantic_search` in health and the configured model directory. |
| Capture appears late | Check host hook enablement, `.cairnignore`, stderr and the [outbox](../../technical/hooks/index.md#outbox). Successful later delivery replays queued events. |
| Automatic recall stays quiet | Search explicitly. [Review the budgets, trust and muting rules](../memory/layers.md#automatic-recall). Check host-specific delivery timing in the [hook matrix](../../technical/hooks/index.md#command-recall-by-host). |
| Jobs fail repeatedly | Review quarantined work in **Activity → Jobs** as the administrator. The worker retries with backoff and surfaces persistent failures. |
| Hermes chat memory is off | Confirm the [profile, host version, server version and bound route](../agents/hermes-gateway.md). Restart the gateway and run `/new` after refreshing the provider. |
