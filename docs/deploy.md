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
| PostgreSQL | a Dokploy PostgreSQL 16 service | The only place Cairn keeps data. Back it up; the containers hold nothing else. |
| `cairn-server` | `ghcr.io/cunilab/cairn-memory-server:vX.Y.Z` | Listens on `0.0.0.0:3111`. Applies its migrations on startup. |
| `cairn-web` | `ghcr.io/cunilab/cairn-memory-web:vX.Y.Z` | Next.js console, listens on port `3000`. |

`latest` also exists, but pin a version tag so an update is a deliberate redeploy.

## Environment

`cairn-server` (the only variable it reads):

| Variable | Value |
| --- | --- |
| `CAIRN_DATABASE_URL` | `postgres://<user>:<password>@<postgres-service>:5432/<database>` |

Without it the server falls back to the JSON file store and needs `--store`; the image's command passes only `--bind 0.0.0.0:3111`, so always set it.

`cairn-web`:

| Variable | Value |
| --- | --- |
| `CAIRN_API_URL` | `http://<server-app>:3111`, the internal address of the server service (read at request time) |

The console's browser calls go to its own `/api/cairn/*` routes, which forward to `CAIRN_API_URL` server to server. The server needs no public route for the console.

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

- CLI and MCP: `cairn setup https://cairn.example.test/agent` (token on stdin), or `CAIRN_URL` with `CAIRN_DEVICE_TOKEN`.
- Hook: `CAIRN_URL=https://cairn.example.test/agent` in the hook's environment ([hook contract](./contracts/hook-v1.md)). A `CAIRN_OUTBOX` queue is bound to that exact base, so changing the prefix needs a new queue file.

A trailing slash is dropped. Create the first account in the console on first visit, then create a device there for each machine ([account rules](./contracts/auth-layer3.md)).

## Limits

- `/agent` exposes the whole server API, not only the agent routes: health, bootstrap, login, and everything else are reachable from the internet. Bootstrap works until the first account exists, so create it right after the first deploy. Sign-in tries are limited per email; device and session tokens are the only other gate.
- The console sets its session cookie `Secure` only when it sees an https request. Check the cookie in a browser after the first sign-in; if it is not `Secure`, Traefik is not passing the scheme through.
- `cairn-server` image tags follow the release; there is no downgrade path for the database, so read the release's migration notes before updating.
