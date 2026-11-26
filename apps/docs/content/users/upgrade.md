---
id: CM-DOC-USERS-UPGRADE
title: "Upgrade Cairn"
status: accepted
description: "Back up data, upgrade matching services and clients, then verify the installation."
---

# Upgrade Cairn

1. Read the [release notes](releases/index.md) for the versions you are crossing. Upgrades from the older 0.1.5 store need a fresh database. [0.1.6 breaking changes](releases/0.1.6.md#breaking-changes)
2. [Back up PostgreSQL](../technical/backups.md). Keep your previous image tags and the backup together. Database migrations run at server startup and there is no database downgrade path.
3. Update the server and console to matching version tags. Check the server's health and confirm the console loads.
4. Run `cairn update --check` to preview the upgrade, then `cairn update` on each device. The updater verifies the checksum and refreshes detected integrations. After updating from 0.1.9, run `cairn setup <agent>` once. Refresh non-default Hermes profiles with their own `HERMES_HOME`.
5. Run `cairn doctor` on each device. Confirm a known project memory still opens in the console and appears in agent search. Follow the version-specific setup and verification steps in the release notes.

[How the client updater works](../technical/release-process.md#how-clients-update).
