---
id: CM-DOC-USERS-INSTALL-SERVER
title: "Install the server"
status: accepted
description: "Run the Cairn server, PostgreSQL and the web console locally."
---

# Install the server

## Run the local stack

Install Docker with Compose. Use the [Compose file in the repository README](../../../../README.md#1-run-the-server-and-web-console) to start PostgreSQL 17 with pgvector, the Cairn server on loopback port 3111 and the console on loopback port 3000.

Save the example as `compose.yaml` and supply a URL-safe `POSTGRES_PASSWORD` in the environment. Start all three services with Compose.

```sh
docker compose up -d
```

The named database volume holds accounts, projects, memories and jobs across service restarts. For an upgrade you control, use matching `vMAJOR.MINOR.PATCH` image tags for server and console.

## Create the first account

Open `http://localhost:3000`. The first account is the only administrator and can create additional accounts. Create a device from **Settings → Devices**, then follow the [CLI install guide](install-cli.mdx).

## Other deployment options

Use the standalone server when you already operate PostgreSQL with pgvector 0.8.0 or newer. The [deployment guide](../technical/deployment.md) covers environment variables, the Linux runtime, remote TLS and the agent API address.
