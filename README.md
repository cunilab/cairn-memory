# Cairn Memory

Shared memory for coding agents and the people who run them: self-hosted, PostgreSQL-backed, and without a server LLM.

Cairn captures useful work and helps agents recall decisions, gotchas and handoffs across sessions and machines. A web console lets people inspect, correct and share project knowledge.

## Features

- Hooks for Claude Code, Codex and OpenCode capture work and provide relevant context.
- Five MCP tools search, get, save, give feedback and forget memories.
- Personal, project and global scopes control visibility.
- Hybrid recall combines full-text, entities, links and local MiniLM embeddings.
- Handoffs and session digests support continuity across work sessions.
- The console shows memories, projects, activity, a graph and actionable Fixes.
- Best-effort redaction runs before storage; upgrades preserve memory data.

## How it works

```text
Agent hook / CLI / MCP  --device token-->  Cairn server  -->  PostgreSQL + pgvector
Web console             --web session-->       ^
```

The server processes capture and recall without an external LLM. See [architecture](docs/architecture.md).

## Quick start

1. Start the server and web console using the [deployment guide](docs/deploy.md) or the local development stack below.
2. Open the web console and create the first account. This account is the administrator.
3. Create a device token in the console.
4. Install the CLI and run `cairn setup https://your-server.example --agent claude`; supply the device token on stdin. Use `codex` or `opencode` for those agents.
5. In a project folder run `cairn init "Project name"`, then commit `.cairn/project.json`. Run `cairn doctor` to check the setup.

Anyone with an account on your server and the project ID can join the project. Protect the project file accordingly. See the [user guide](docs/user-guide.md).

## Install the CLI

Download the `cairn-<target>` asset from the [GitHub releases](https://github.com/cunilab/cairn-memory/releases), verify it against `SHA256SUMS`, make it executable where needed and put it on your `PATH`. CLI targets are macOS arm64, Linux x86-64 and arm64 (musl), and Windows x86-64. Server Linux binaries use glibc; container images are available for amd64 and arm64.

```sh
cairn --version
cairn update --check
cairn update
```

See [releases and updates](docs/release.md) for details and security limits.

## Documentation

| Guide | Covers |
| --- | --- |
| [User guide](docs/user-guide.md) | Setup, daily use, projects, Fixes and privacy |
| [Architecture](docs/architecture.md) | Components, data flows and decisions |
| [CLI and MCP](docs/reference/cli-and-mcp.md) | Commands, tools and records |
| [Hook](docs/reference/hook.md) | Capture, redaction, setup and outbox |
| [Projects](docs/reference/projects.md) | Scopes, membership and trust |
| [Recall](docs/reference/recall.md) | Ranking, jobs and handoffs |
| [Authentication](docs/reference/auth.md) | Accounts, devices and admin API |
| [Deployment](docs/deploy.md) | Server, console and reverse proxy |
| [Development](docs/development.md) | Build, checks, migrations and docs |
| [Release notes](docs/release-notes-0.1.9.md) | Version-specific changes and upgrade steps |

## Development

```sh
./scripts/dev_up.sh
cargo test --workspace
./scripts/smoke_mcp.sh && ./scripts/smoke_web.sh
```

See [development](docs/development.md) for database requirements, checks and how to stop the development stack.

## Status

Cairn is pre-1.0. Read the release notes for your version and upgrade the server and clients together. Current migrations preserve existing memory data.

## Acknowledgements and licence

Cairn draws inspiration from [AgentMemory](https://github.com/rohitg00/agentmemory) and Graphify. Cairn Memory is licensed under the [Apache License 2.0](LICENSE).
