<div align="center">
  <img src="docs/assets/cairn-logo.svg" alt="Cairn logo" width="96">
  <h1>Cairn Memory</h1>
  <p>Shared memory for coding agents and the people who run them.</p>

  <a href="https://github.com/cunilab/cairn-memory/actions/workflows/ci.yml"><img src="https://github.com/cunilab/cairn-memory/actions/workflows/ci.yml/badge.svg" alt="CI status"></a>
  <a href="https://github.com/cunilab/cairn-memory/releases/latest"><img src="https://img.shields.io/github/v/release/cunilab/cairn-memory" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/licence-Apache--2.0-blue.svg" alt="Apache 2.0 licence"></a>
</div>

## What is Cairn

Coding agents forget decisions and gotchas between sessions, machines and teammates. Cairn captures useful work and brings it back when an agent needs it, while a web console lets people inspect, correct and share knowledge. It is self-hosted for individuals and small teams, uses PostgreSQL as its authoritative store, and runs without a server LLM. Cairn preserves memories across upgrades and keeps agent context compact; token costs depend on the work and results shown.

- Capture prompts, edits and commands; distill decisions, gotchas and session handoffs.
- Search across personal, project and global scopes with full-text, entities, links and local embeddings.
- Inspect memories, relationships, sessions and actionable Fixes in the web console.
- Bring existing Claude Code memories into Cairn and keep the original files on disk.

```text
Agent hooks / CLI / MCP  ──device token──▶  Cairn server  ──▶  PostgreSQL + pgvector
Web console              ──web session───▶       │
                                                 └── capture → distill → recall
```

| Agent | Setup | Integration |
| --- | --- | --- |
| Claude Code | `--agent claude` | Hooks and MCP |
| Codex | `--agent codex` | Hooks and MCP; trust hooks in Codex |
| OpenCode | `--agent opencode` | Plugin hooks and MCP; no continuing Stop reminder |
| Grok Build | `--agent grok` | Hooks and MCP with a brief on the first tool result |
| Antigravity CLI | `--agent antigravity` | Hooks and MCP with checkpoints on Stop |
| Hermes Agent | `--agent hermes` | Memory provider with native Cairn tools, plus tool hooks; Hermes v2026.4.23 or newer |

## Set up Cairn

### 1. Run the server and web console

You need Docker and Compose for this example, or PostgreSQL with pgvector 0.8.0 or newer and a standalone server. Development and CI use PostgreSQL 17 with `pgvector/pgvector:pg17`. Server images support Linux amd64 and arm64; the CLI supports macOS arm64, Linux x86-64 and arm64, and Windows x86-64.

For a local trial, save the following as `compose.yaml`, choose a database password, and run `docker compose up -d`. The database volume holds your data. The example uses the published `latest` tags; pin matching release tags for a deliberate update.

```yaml
services:
  db:
    image: pgvector/pgvector:pg17
    environment:
      POSTGRES_USER: cairn
      POSTGRES_DB: cairn
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD}
    volumes:
      - cairn_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U cairn -d cairn"]
      interval: 5s
      timeout: 5s
      retries: 20
  server:
    image: ghcr.io/cunilab/cairn-memory-server:latest
    environment:
      CAIRN_DATABASE_URL: postgres://cairn:${POSTGRES_PASSWORD}@db:5432/cairn
    depends_on:
      db:
        condition: service_healthy
    ports:
      - "127.0.0.1:3111:3111"
  web:
    image: ghcr.io/cunilab/cairn-memory-web:latest
    environment:
      CAIRN_API_URL: http://server:3111
    depends_on:
      - server
    ports:
      - "127.0.0.1:3000:3000"
volumes:
  cairn_data:
```

Set `POSTGRES_PASSWORD` in your environment before starting Compose; use a URL-safe password in this example. Open `http://localhost:3000` and create the first account immediately: it is the only administrator. The administrator creates additional accounts. The server also accepts `CAIRN_PUBLIC_URL`, `CAIRN_SCHEMA` and `CAIRN_EMBEDDING_MODEL_DIR`; the console uses `CAIRN_API_URL` for its internal server connection. For remote access, TLS, reverse-proxy routing, backups and deployment limits, follow the [deployment guide](docs/deploy.md). Agents accept HTTP only for loopback hosts.

### 2. Install and configure the first device

Copy and paste the block for your platform. It downloads the latest release, verifies it against `SHA256SUMS`, installs it as `cairn` and prints the version.

macOS (Apple silicon):

```sh
base=https://github.com/cunilab/cairn-memory/releases/latest/download
asset=cairn-aarch64-apple-darwin
curl -fLO "$base/$asset"
curl -fLO "$base/SHA256SUMS"
grep "  $asset$" SHA256SUMS | shasum -a 256 -c -
chmod +x "$asset"
mkdir -p ~/.local/bin && mv "$asset" ~/.local/bin/cairn
cairn --version
```

Linux (x86-64 or arm64; `uname -m` picks the right binary):

```sh
base=https://github.com/cunilab/cairn-memory/releases/latest/download
asset=cairn-$(uname -m)-unknown-linux-musl
curl -fLO "$base/$asset"
curl -fLO "$base/SHA256SUMS"
grep "  $asset$" SHA256SUMS | sha256sum -c -
chmod +x "$asset"
mkdir -p ~/.local/bin && mv "$asset" ~/.local/bin/cairn
cairn --version
```

If `cairn` is not found, add `~/.local/bin` to `PATH`, for example `echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc` (or `~/.zshrc`), and open a new terminal.

Windows (x86-64, PowerShell):

```powershell
$base = "https://github.com/cunilab/cairn-memory/releases/latest/download"
$asset = "cairn-x86_64-pc-windows-msvc.exe"
Invoke-WebRequest "$base/$asset" -OutFile $asset
Invoke-WebRequest "$base/SHA256SUMS" -OutFile SHA256SUMS
$expected = ((Select-String -Path SHA256SUMS -Pattern $asset -SimpleMatch).Line -split '\s+')[0]
if ((Get-FileHash $asset -Algorithm SHA256).Hash -ne $expected) { throw "SHA256 mismatch for $asset" }
$dir = "$env:LOCALAPPDATA\cairn\bin"
New-Item -ItemType Directory -Force $dir | Out-Null
Move-Item -Force $asset "$dir\cairn.exe"
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$dir*") { [Environment]::SetEnvironmentVariable("Path", "$userPath;$dir", "User") }
& "$dir\cairn.exe" --version
```

Open a new PowerShell window afterwards so `cairn` is on `PATH`.

Alternatively build from source with `cargo build --release -p cairn-client`; the binary is `target/release/cairn` (or `cairn.exe` on Windows).

In the web console, create a device token for this machine. Run setup with your server's agent API base URL, then supply the token on stdin when prompted; do not put it in the command line:

```sh
cairn setup http://localhost:3111 --agent claude
cairn doctor
```

Use `codex`, `opencode`, `grok`, `antigravity` or `hermes` for those agents. Codex and OpenCode setup need their native CLIs on `PATH`. Hermes must be v2026.4.23 or newer; setup warns about an older or unknown version. Codex also requires trusting the installed hooks. Once the device is configured, `cairn setup <agent>` uses the saved device configuration. Setup manages the hooks, MCP entries and a block in the agent's global instructions. See [hook setup](docs/reference/hook.md#setup).

In an existing Claude Code project, run `cairn import claude-memory` to preview and confirm an import. Cairn backs up `MEMORY.md` before replacing it with a pointer. Claude setup also blocks writes to its native memory directory. Add `--keep-native-memory` to the Claude setup command to omit the write guard. See [Claude memory import](docs/reference/cli-and-mcp.md#import-claude-code-memory).

### 3. Add a device, agent or teammate

Create a separate device token in the web for each machine, install the CLI there, and run `cairn setup <server-url> --agent <agent>` with that token on stdin. Run setup for each agent you use on that machine. A teammate signs into their own account, creates their own device token and configures their agent the same way.

In the project folder on the first device run `cairn init "Project name"` and commit `.cairn/project.json`. Teammates with an account on the same server get that file in their checkout and run `cairn init` to join. **Anyone with a server account and the project ID can join and read project memories and member emails**; protect the file accordingly. Project memories are shared with members, personal memories remain private to their owner, and global memories are visible to authenticated server users. See [projects and scopes](docs/reference/projects.md).

### 4. Update

Back up PostgreSQL and upgrade the server and clients together. Migrations run at server startup. Run `cairn update --check` to check for an update or `cairn update` to install it and refresh configured integrations. Run `cairn setup <agent>` to refresh one integration using the saved device configuration. Run `cairn doctor` and consult the [release notes](docs/release-notes-0.1.10.md) before upgrading. There is no database downgrade path.

## Use Cairn

At session start the hook requests a brief and an instruction. During work it captures prompts, edits and non-read-only commands; a worked session can produce reusable notes and a handoff. In an eligible project session, the Stop hook may ask the agent to save useful notes. Agents can search as needed or fetch a full memory. If a project owner renames a project in the web, teammates see a session-start notice; run `cairn init` to update the local name and commit the file.

| MCP tool | Purpose |
| --- | --- |
| `memory_search` | Find relevant memories. |
| `memory_get` | Fetch full records by ID. |
| `memory_save` | Save a decision, gotcha, procedure or other reusable knowledge. |
| `memory_feedback` | Mark a memory helpful or wrong. |
| `memory_forget` | Hide an owned memory from normal recall. |
| `skill_search` | Find shared skills. |
| `skill_get` | Read one version of a skill. |
| `skill_save` | Create a skill or publish a new version. |
| `skill_feedback` | Vote on a skill version. |

The CLI also offers `cairn search`, `cairn get`, `cairn save`, `cairn feedback` and `cairn forget`. Use `cairn fixes` to list device fixes, `cairn fixes apply [id]` to apply them, and `cairn doctor` to diagnose setup. See [CLI and MCP](docs/reference/cli-and-mcp.md) for arguments.

In the web console:

- **Dashboard** shows activity and health.
- **Memories** lists and inspects records.
- **Skills** lists shared instructions and their version history, and suggests procedures to publish as skills.
- **Projects** shows members, branches and session reports.
- **Graph** shows memory relationships.
- **Activity** shows sessions, jobs and audit events.
- **Fixes** offers evidence and actions for detected issues.
- **Settings** manages the account, devices and agent instructions.

### Privacy and configuration

Cairn runs on your infrastructure. Redaction before persistence is best effort, not a guarantee: avoid sharing secrets and rotate any exposed credential. Administrators can inspect redacted recent search phrases and rewrite memories; project membership and scope determine visibility. Raw observations are retained for processing and expire separately from memories. Read the [privacy guide](docs/user-guide.md#privacy-and-data).

The CLI, MCP server and hook use the selected config file: `CAIRN_CONFIG`, otherwise `$XDG_CONFIG_HOME/cairn/config.json`, otherwise `~/.config/cairn/config.json` (or the Windows app-data location). `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` are ignored. Set `nudge` to `false` in the client config or `CAIRN_NO_NUDGE=1` to disable reminders; assistant messages are captured only with `CAIRN_CAPTURE_ASSISTANT=1`. Set `CAIRN_SESSION_ORIGIN=automation` for automated sessions. A `.cairnignore` file disables hook capture for the checkout. See [hook details](docs/reference/hook.md).

### Troubleshooting

- **Agent does not connect:** run `cairn doctor`, check the selected config and device status, and rerun setup with the token.
- **Project is not shared:** commit `.cairn/project.json`, verify the teammate uses the same server and run `cairn init` there.
- **Semantic search is disabled:** check the model directory and server health; lexical, entity and link retrieval remain available.
- **Events are delayed:** inspect hook stderr, the outbox and Activity for failed or quarantined jobs.

## Development

For contributors, the local development stack uses Docker. The project instructions and commands are in [AGENTS.md](AGENTS.md) and [development](docs/development.md).

```sh
./scripts/dev_up.sh
cargo test --workspace
./scripts/smoke_mcp.sh && ./scripts/smoke_web.sh
python3 scripts/validate_phase0.py
```

Cairn is pre-1.0. Use disposable databases for tests and read the migration rules before changing storage.

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
| [Release process](docs/release.md) | Releases and updates |
| [Release notes](docs/release-notes-0.1.10.md) | Version-specific changes and upgrade steps |

## Acknowledgements and licence

Cairn draws inspiration from [AgentMemory](https://github.com/rohitg00/agentmemory) and Graphify. Cairn Memory is licensed under the [Apache License 2.0](LICENSE).
