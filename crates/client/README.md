# cairn-client

## What
The agent side: the `cairn` CLI and the `cairn mcp` stdio server. Both call the REST API with a device
token from one shared tool table; the crate holds no account session and opens no store. It also finds
`.cairn/project.json`, installs the hooks, configures agents and runs `cairn update`.

## Entry points
- `bin/cairn.rs` (commands), `mcp::run_stdio`, `tools` (the five-tool table: `tools/list`, CLI verbs,
  instruction text, REST mapping).
- `present` (MCP result envelope), `doctor` (read-only health checks), `hook` (stub that runs the
  installed Python hook), `project` (project file and repository identity).
- `agents::configure`, `update` (verified self-update), `Config`, `Client`.

## Depends on
`ureq` over HTTP to `cairn-server`. Tests also use `cairn-server` and `cairn-store` (feature `testing`).
