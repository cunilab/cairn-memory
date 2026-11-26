# cairn-client

## What
The agent side: the `cairn` CLI and the `cairn mcp` stdio server. Both call the REST API with a device
token from one shared tool table; the crate holds no account session and opens no store. It also finds
`.cairn/project.json`, runs `cairn hook` (capture and injection), configures agents and runs `cairn update`.

## Entry points
- `bin/cairn.rs` (commands), `mcp::run_stdio`, `tools` (the memory and skill tool table: `tools/list`, CLI verbs,
  instruction text, REST mapping).
- `present` (MCP result envelope), `doctor` (read-only health checks), `hook` (`cairn hook <Event>`: events, outbox,
  state, injection), `project` (project file and repository identity).
- `fixes` (`cairn fixes`), `setup::configure` (agent MCP and hook installation), `update` (verified self-update, then
  `setup::refresh`), `Config`, `Client`.

## Depends on
`ureq` over HTTP to `cairn-server`. Tests also use `cairn-server` and `cairn-store` (feature `testing`).
