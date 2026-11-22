---
id: CM-DEPLOY-001
title: "Cairn Memory Deployment Layout"
status: draft
---

# Cairn Memory Deployment Layout

One host, one domain: the web console and the agent API share `https://cairn.example.test`. The layout below is a Dokploy setup behind Traefik. It describes the intended shape and has not been run end to end; the release workflow that publishes the images is [release](./release.md).

## Services

| Service | Image or source | Notes |
| --- | --- | --- |
| PostgreSQL | PostgreSQL with pgvector (`pgvector/pgvector:pg17` in development/CI) | The canonical store; back it up. The migration needs permission to create the `vector` extension, or an operator must install it first. |
| `cairn-server` | `ghcr.io/cunilab/cairn-memory-server:vX.Y.Z` | Listens on `0.0.0.0:3111`. Applies its migrations on startup. |
| `cairn-web` | `ghcr.io/cunilab/cairn-memory-web:vX.Y.Z` | Next.js console, listens on port `3000`. |

`latest` also exists, but pin a version tag so an update is a deliberate redeploy.

## Environment

`cairn-server`:

| Variable | Value |
| --- | --- |
| `CAIRN_DATABASE_URL` | `postgres://<user>:<password>@<postgres-service>:5432/<database>` |
| `CAIRN_EMBEDDING_MODEL_DIR` | `/opt/cairn/models/minilm` by default in the image; a provisioned directory for standalone servers |
| `CAIRN_PUBLIC_URL` | Optional, recommended: the address agents are given, e.g. `https://cairn.example.test/agent`. The console shows it in the device setup command and in every fix command (`cairn setup <url>`). Without it the console forwards the address the browser used (`x-forwarded-host` and `-proto`, plus `x-forwarded-prefix: /agent` for a public host) and the server answers with that, so a deploy that follows the routing below still shows `https://cairn.example.test/agent`. A console on `localhost` forwards nothing, and the server answers with the address a request reached it on |

The server exits at startup without it (`missing --database-url <url> or CAIRN_DATABASE_URL`); the image's command passes only `--bind 0.0.0.0:3111`, so always set it.

The production server image uses Debian trixie slim for the CPU ONNX runtime,
includes the pinned MiniLM model, and targets `linux/amd64` and `linux/arm64`.
It makes no model downloads at runtime. Missing or invalid model files leave
`semantic_search: disabled` in health with lexical/entity/link recall available;
see [hybrid recall](contracts/continuity-v1.md#hybrid-recall-v014).

`cairn-web`:

| Variable | Value |
| --- | --- |
| `CAIRN_API_URL` | `http://<server-app>:3111`, the internal address of the server service (read at request time) |

The console's browser calls go to its own `/api/cairn/*` routes, which forward to `CAIRN_API_URL` server to server. The server needs no public route for the console. `CAIRN_API_URL` is internal: it never appears in a setup command, because the console forwards the browser's host, not its own.

## Routing

Traefik sends two routes to two services:

| Rule | Service | Middleware |
| --- | --- | --- |
| ``Host(`cairn.example.test`) && PathPrefix(`/agent`)`` | `cairn-server` port `3111` | `stripprefix` with prefix `/agent` |
| ``Host(`cairn.example.test`)`` | `cairn-web` port `3000` | none |

The `/agent` router needs a higher priority than the catch-all host router (Traefik ranks by rule length, so the longer rule already wins; set `priority` explicitly if you want it to be obvious). The server keeps serving `/cairn-memory/...`; clients reach it as `https://cairn.example.test/agent/cairn-memory/...`. The console keeps `/api/*`, so the two never collide.

Terminate TLS at Traefik. Agents refuse plain `http` for any host that is not loopback.

## Agents

Every agent client takes the base URL with the path prefix:

- CLI and MCP: `cairn setup https://cairn.example.test/agent` (token on stdin). `CAIRN_CONFIG` selects the file; otherwise setup uses the default config path.
- Hook: the same file, or `--config` for an installed hook ([hook contract](./contracts/hook-v1.md)). The hook outbox is bound to that exact base, so changing the prefix starts a new queue file.

A trailing slash is dropped. Create the first account in the console on first visit, then create a device there for each machine ([account rules](./contracts/auth-layer3.md)).

## Limits

- `/agent` exposes the whole server API, not only the agent routes: health, bootstrap, login, and everything else are reachable from the internet. Bootstrap works until the first account exists, so create it right after the first deploy. There is no sign-in rate limit or lockout: guessing a password costs one Argon2 verify per try (about 50 to 100 ms on one core), and passwords are at least 12 characters. Add a limit in the reverse proxy if the console is public; device and session tokens are the only other gate.
- The console sets its session cookie `Secure` only when it sees an https request. Check the cookie in a browser after the first sign-in; if it is not `Secure`, Traefik is not passing the scheme through.
- `cairn-server` image tags follow the release; there is no downgrade path for the database, so read the release's migration notes before updating.
