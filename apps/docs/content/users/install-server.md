---
id: CM-DOC-USERS-INSTALL-SERVER
title: "Install the server"
status: accepted
description: "Run the Cairn server, PostgreSQL and the web console locally."
---

# Install the server

Deploy the Cairn server, PostgreSQL 17 with pgvector, and the web console using Docker Compose.

## Run the local stack

Prerequisite: Docker with Compose. Use the [Compose file in the repository README](../../../../README.md#1-run-the-server-and-web-console).

Save the file as `compose.yaml`, set a URL-safe `POSTGRES_PASSWORD` in your environment, and start all three services:

```sh
docker compose up -d
```

| Service | Port | Description |
| --- | --- | --- |
| `cairn-server` | `3111` | REST API and background workers |
| `cairn-web` | `3000` | Next.js web console |
| `postgres` | `5432` | PostgreSQL 17 with `pgvector` extension |

A named database volume retains accounts, projects, memories, and jobs across container restarts. For controlled upgrades, use matching `vMAJOR.MINOR.PATCH` image tags for the server and console.

## Create the first account

1. Open `http://localhost:3000` in your browser.
2. Register the initial account. This first account is the sole administrator and can invite teammates.
3. Navigate to **Settings → Devices** to create a device token.
4. Proceed to [Install the CLI](install-cli.mdx).

## Other deployment options

For standalone servers with an external PostgreSQL 17 + pgvector 0.8.0+ instance, consult the [deployment guide](../technical/deployment.md) for environment variables, Linux runtime setups, and reverse proxy TLS configurations.
