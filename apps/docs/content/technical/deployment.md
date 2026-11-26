---
id: CM-DEPLOY-001
title: "Cairn Memory Deployment Layout"
status: draft
description: "Deploy the server and console, configure pgvector, and route agents through Traefik."
---

# Cairn Memory Deployment Layout

Production deployment specification for server, web console, PostgreSQL, and Traefik reverse proxy.

## Services

Production deployment architecture uses three core services on a single domain (e.g. `https://cairn.example.com`):

| Service | Container image | Port | Purpose |
| --- | --- | --- | --- |
| `postgres` | `pgvector/pgvector:pg17` | `5432` | Authoritative store with `pgvector` extension |
| `cairn-server` | `ghcr.io/cunilab/cairn-memory-server:vX.Y.Z` | `3111` | Axum REST API and background workers |
| `cairn-web` | `ghcr.io/cunilab/cairn-memory-web:vX.Y.Z` | `3000` | Next.js 16 web console |

## Environment

### `cairn-server` configuration

| Variable | Description | Example |
| --- | --- | --- |
| `CAIRN_DATABASE_URL` | PostgreSQL connection URL (mandatory) | `postgres://user:pass@postgres:5432/cairn` |
| `CAIRN_EMBEDDING_MODEL_DIR` | Directory containing MiniLM ONNX model | `/opt/cairn/models/minilm` |
| `CAIRN_SCHEMA` | PostgreSQL schema name (defaults to `cairn`) | `cairn` |
| `CAIRN_PUBLIC_URL` | Canonical agent endpoint displayed in console setup | `https://cairn.example.com/agent` |

### `cairn-web` configuration

| Variable | Description | Value |
| --- | --- | --- |
| `CAIRN_API_URL` | Internal server address for proxying requests | `http://cairn-server:3111` |

## Routing

Configure your reverse proxy (e.g. Traefik) to route traffic on the public domain:

| Priority | Match rule | Destination | Middleware |
| --- | --- | --- | --- |
| **High** | `Host(cairn.example.com) && PathPrefix(/agent)` | `cairn-server:3111` | Strip prefix `/agent` |
| **Default** | `Host(cairn.example.com)` | `cairn-web:3000` | None |

TLS must be terminated at the reverse proxy. Agent clients reject plain HTTP connections to non-loopback addresses.

## Agents

Agent connection commands use the routed agent path:

```sh
cairn setup https://cairn.example.com/agent
```

## Limits

- **Endpoint exposure**: The `/agent` route exposes the REST API. Ensure the initial administrator account is registered immediately upon deployment to disable the bootstrap route.
- **Brute force protection**: Authentication uses Argon2 hashing (~50–100ms per attempt). Enforce IP rate limiting in your reverse proxy for public endpoints.
- **Secure cookies**: Web session cookies enforce the `Secure` flag on HTTPS connections. Ensure the reverse proxy forwards `X-Forwarded-Proto: https`.
- **Database upgrades**: Schema migrations run forward only. Always back up the database before upgrading images.
