---
id: CM-DOC-USERS-UPGRADE
title: "Upgrade Cairn"
status: accepted
description: "Back up data, upgrade matching services and clients, then verify the installation."
---

# Upgrade Cairn

Safely upgrade the Cairn server, console, and client devices across release versions without data loss.

1. **Review release notes**: Read the [release notes](releases/index.md) for all intermediate versions. Upgrading from the legacy 0.1.5 store requires a fresh database ([0.1.6 breaking changes](releases/0.1.6.md#breaking-changes)).
2. **Back up database**: Run a [PostgreSQL backup](../technical/backups.md) before updating. Migrations run automatically on server startup; there is no downgrade path.
3. **Upgrade server & console**: Pull matching version tags for `cairn-server` and `cairn-web`. Restart containers and verify health endpoints.
4. **Update client devices**: Preview updates with `cairn update --check`, then run `cairn update` on each machine.
   - The updater verifies binary checksums and refreshes detected agent configurations.
   - When updating from 0.1.9, run `cairn setup <agent>` once.
   - Non-default Hermes profiles must be refreshed with their specific `HERMES_HOME`.
5. **Verify with doctor**: Run `cairn doctor` on each machine. Confirm that project memories appear in the console and resolve in agent search queries.

See [How the client updater works](../technical/release-process.md#how-clients-update).
