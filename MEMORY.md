# Cairn Memory — Agent Memory

This file contains a small set of durable, project-specific facts for future
coding-agent sessions. It is intentionally not a diary, transcript, backlog, or
replacement for the project documentation.

## How to use this file

- Read it before substantive work.
- Save only verified knowledge that is likely to matter again.
- Keep entries short and link to the authoritative source.
- Update or supersede an existing entry when the project changes.
- Treat current code and `docs/` as authoritative if anything conflicts here.
- Never store secrets, tokens, private payloads, or temporary task progress.

## Durable project context

### Personal ownership and continuity come first

- **Type:** product boundary
- **Status:** active
- **Summary:** Cairn is an AI-managed, person-owned memory layer. Personal
  memory and branch continuity are delivered before project promotion,
  collaboration, and federation.
- **Reason:** Each layer must provide a usable, trustworthy product without
  requiring the complete platform.
- **Evidence:** `README.md`; `docs/product.md`; `docs/roadmap.md`

### Documentation is evidence-driven

- **Type:** workflow rule
- **Status:** active
- **Summary:** Claims about implementation or verification need a code reference
  and a recorded check. Contract, storage, trust-boundary, retrieval, and sync
  changes require an ADR or ADR update.
- **Reason:** Cairn’s memory and ownership boundaries must remain inspectable and
  explainable as the system evolves.
- **Evidence:** `docs/standards.md`

### Self-hosted account authentication is the first Layer 3 model

- **Type:** decision
- **Status:** active
- **Summary:** The first production-oriented identity boundary uses self-hosted accounts, Argon2id password hashes, hashed 30-day bearer sessions, and account-authorized device registration. OIDC remains a later adapter.
- **Reason:** This is the smallest complete identity path for the self-hosted target and keeps account ownership separate from device sync credentials.
- **Evidence:** `docs/adr/0008-self-hosted-account-authentication.md`; `docs/contracts/auth-layer3.md`; `crates/core/src/lib.rs`; `crates/server/src/lib.rs`; `cargo test --workspace`

### 2026-11-14 — Device tokens are agent credentials; the web session is the human surface

- **Type:** decision
- **Status:** active
- **Summary:** Humans use only the web console (account session). Agents use a device token that only a web session creates, lists, rotates and revokes (shown once). A device token is accepted on every agent route and acts as its owning account in both routers; sync stays device-only. It gets `403` (`WEB_ONLY_ERROR`) on bootstrap, register, login, logout, audit, device management, and branch merge on a project with any member besides its owner. Revoked token is `401`. `GET /health` reports `bootstrap_required` for the console's first-account form. The hook is device-token only (`CAIRN_ACCOUNT_TOKEN` and anonymous observe removed). A new file `sync_push` now records the device person as observation owner, otherwise account reads and session end cannot see hook captures. CLI and MCP are local-store tools and were not changed. Supersedes the earlier hook transport entries that send an account token or no token to observe.
- **Reason:** Automation-first: an agent holding the account bearer could not be told apart from a person, so merge gating and device control had no signal.
- **Evidence:** `docs/adr/0008-self-hosted-account-authentication.md` (2026-11-14 amendment); `docs/contracts/auth-layer3.md`; `crates/core/src/lib.rs`; `crates/storage/src/lib.rs`; `crates/server/src/lib.rs`; `cargo test --workspace`; `scripts/smoke_layer3.sh`; `scripts/smoke_web.sh`. The PostgreSQL test and `scripts/smoke_postgres.sh` were not run (database container I/O errors).

### 2026-11-13 — The remote Cairn server is canonical; local outbox and file backend are not

- **Type:** decision
- **Status:** active
- **Summary:** The Cairn server is the canonical source of truth for accepted memory; PostgreSQL (`cairn-server --database-url`, `cairn-storage`) is its canonical database and the cross-device sync path. A local spool, outbox or cache is only a bounded delivery bridge, never a second canonical store. The standalone file backend (`--store`) is Layer 1/2 compatibility and development, not the intended cross-device architecture, and no managed Cairn cloud exists: the server is self-hosted. Implemented bridge today (distinct from the intent): an opt-in, device-token-only, bounded (256) outbox with manual replay that removes entries only after server acknowledgement. Account-token and anonymous capture cannot queue offline. There is no replay worker and no local read cache.
- **Reason:** One permissioned source of truth serves many people, agents and devices; a client-local canonical store would need conflict-prone sync between canonical stores. The user corrected an earlier reading that treated local and remote as equally canonical.
- **Evidence:** `docs/adr/0003-server-authoritative-postgres.md` (decisions 1, 2 and 6); `docs/product.md` (server authoritative, edge tolerant); `docs/architecture.md`; `docs/contracts/sync-layer3.md` (local outbox); `docs/contracts/hook-v1.md`; `scripts/cairn_hook.py`; `scripts/cairn_outbox.py`; `crates/server/src/main.rs`; `crates/storage/tests/postgres.rs`; `scripts/smoke_postgres.sh`

## Lessons and recurring corrections

Add only durable lessons here. Corrections to the agent’s working approach
should include the reason they matter and the evidence that supports them.

### 2026-11-03 — Scope sync duplicate checks by person

- **Type:** lesson
- **Status:** active
- **Summary:** A sync event ID is not globally safe for duplicate acknowledgement. A duplicate must first resolve to a persisted sync event owned by the authenticated person; collisions outside that scope must be rejected.
- **Reason:** A global observation lookup previously allowed another person’s device to receive the existing observation when it reused the same event ID.
- **Evidence:** `crates/core/src/lib.rs`; `crates/server/src/lib.rs`; `cargo test --workspace`; `./scripts/smoke_layer3.sh`

### 2026-11-12 — Require file_bearer for file HTTP capture

- **Type:** lesson
- **Status:** active
- **Summary:** Once the file store has any account, session start, observe, session end, and remember use `file_bearer`. A missing or invalid bearer is 401 and does not write. Observe calls `capture_for_account`, end calls `end_session_for_account`, remember calls `remember_for_account`, and start calls `start_session_for_account`. A store with no accounts stays on the unscoped Layer 1 methods. The default hook sends no bearer, so it receives 401 after bootstrap. Superseded 2026-11-12 for hook transport: `CAIRN_ACCOUNT_TOKEN` is an observe bearer only. See the hook transport entry. Account file sessions are separate rows for one public session id.
- **Reason:** An optional bearer used to fall through to the unscoped capture and end methods. That mixed both accounts into an unowned summary and could return another account's observation.
- **Evidence:** `crates/server/src/lib.rs`; `docs/adr/0008-self-hosted-account-authentication.md`; `docs/contracts/auth-layer3.md`; `docs/contracts/hook-v1.md`; `docs/audits/2026-11-12-memory-capture.md`; `cargo test -p cairn-server --lib -- file_http_capture_uses_file_bearer_when_accounts_exist`

### 2026-11-12 — Roll back a capture when its file rename fails

- **Type:** lesson
- **Status:** active
- **Summary:** A file capture acknowledgment means the rename completed. `capture_observation` publishes the observation in that rename. `capture_for_account` publishes the observation and owner record in the same rename. `sync_push` publishes a new observation and its sync event in the same rename. If the rename fails, those staged rows are removed and the call returns the IO error. A retry writes. An owned or unscoped duplicate does not write again. A sync duplicate still persists device last-seen. An unowned event that already exists outside sync is still rejected, and another person's event id is still rejected. An owned observation with no sync event follows the adoption rule below. The rename is not followed by `fsync`.
- **Reason:** Pushing the row before a failed rename left it in memory. The retry acknowledged a duplicate, and a restart loaded the previous file without that row. A sync push that persisted the observation and then failed the sync-event write left an observation with no sync metadata.
- **Evidence:** `crates/core/src/lib.rs`; `docs/audits/2026-11-12-memory-capture.md`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/adr/0008-self-hosted-account-authentication.md`; `cargo test -p cairn-core --lib`

### 2026-11-12 — Adopt one sync event for an owned file observation

- **Type:** decision
- **Status:** active
- **Summary:** File `sync_push` attaches exactly one sync event to an existing observation that has no sync event when `observation_owners` shows the authenticated device's person owns it. The sync event copies the stored observation. Request content, session, context, and person text are not ownership and do not replace the row. The first attachment returns `duplicate: false` and a new cursor. A later push of that person's persisted sync event returns `duplicate: true` and the same cursor. A failed rename drops the new sync event, cursor, and device last-seen change and leaves the observation and owner. An unowned observation stays `event already exists outside sync scope`. A foreign owner or foreign sync event stays `event belongs to another person` with no body and no new owner. A new empty-context push copies that device person's session row. The file still has one observation per event id. PostgreSQL does not use this file adoption rule. Its empty-context ordering is the later entry on this date.
- **Reason:** Account observe stored the event without a sync row, so a later device push was rejected and another device never pulled it. Adopting every existing id would let a device claim an anonymous or foreign row.
- **Evidence:** `crates/core/src/lib.rs`; `crates/server/src/lib.rs`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/contracts/sync-layer3.md`; `docs/audits/2026-11-12-memory-capture.md`; `cargo test -p cairn-core --lib`; `cargo test -p cairn-server --lib -- file_sync_adopts_an_owned_observation_once`; `cargo clippy -p cairn-core -p cairn-server --all-targets -- -D warnings`

### 2026-11-12 — PostgreSQL empty context follows the session without a new observe route

- **Type:** decision
- **Status:** active
- **Summary:** PostgreSQL capture stays on device `POST /cairn-memory/sync/push`. There is no PostgreSQL `/observe` route. An account token starts and ends sessions. A device token pushes. An exactly empty push copies that person's existing session before normalize. A partial context stays as sent. A missing session is inserted empty so the observation foreign key holds. A later account start fills a session when merging its stored object over the canonical empty context still equals that object, including the column default `{}` and `{"files":[]}`. A real field or an extra key stays. The first non-empty context stays, including when two starts race. `ended_at` is not reopened. Session end copies the session context only onto in-memory observations that are still empty, and only while building the summary. Stored observations and sync payloads are not rewritten. Import without a sync event stays outside sync. Superseded 2026-11-12 for hook delivery: the default hook still posts `/observe`. `CAIRN_DEVICE_TOKEN` posts `sync/push` and does not enqueue. See the hook transport entry.
- **Reason:** An empty push created the session with `ON CONFLICT DO NOTHING`, so a later start could not repair branch, worktree, or commit, and the summary context stayed empty.
- **Evidence:** `crates/storage/src/lib.rs`; `crates/storage/tests/postgres.rs`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/contracts/sync-layer3.md`; `docs/contracts/auth-layer3.md`; `docs/contracts/hook-v1.md`; `docs/audits/2026-11-12-memory-capture.md`; `cargo test -p cairn-storage --test postgres -- --ignored postgres_context_order_repairs_empty_session_without_rewriting_observations` on disposable database `cairn_ctx_order_20261112_0c590134`, then dropped. The empty-object predicate was rechecked on `cairn_ctx_pred_20261112_5f390110`, which was also dropped

### 2026-11-12 — Key file account sessions by person and public session id

- **Type:** decision
- **Status:** active
- **Summary:** Choice B is the file-adapter rule. An account session row is `(person_id, session_id)` and the public session id is unchanged. The same account's second start keeps the first context. Another account gets its own row. A missing `person_id` stays the unscoped row and is not adopted. `STORE_VERSION` stays 1. CLI, MCP, and a store with no accounts stay on that unscoped row. Account import rejects another person's `person_id`. Unscoped import merges on the pair. Already stored observation context is not rewritten.
- **Reason:** Matching only on session id returned the first account's context, copied it into the second account's empty capture, and stamped one `ended_at`. PostgreSQL already uses `(person_id, session_id)`.
- **Evidence:** `crates/core/src/lib.rs`; `crates/server/src/lib.rs`; `docs/adr/0008-self-hosted-account-authentication.md`; `docs/contracts/auth-layer3.md`; `docs/audits/2026-11-12-memory-capture.md`; `cargo test --lib -p cairn-core -p cairn-server`; `cargo clippy -p cairn-core -p cairn-server --all-targets -- -D warnings`; `./scripts/smoke_rest.sh`; `./scripts/smoke_hook.sh`

### 2026-11-12 — Outbox replay must not follow redirects

- **Type:** lesson
- **Status:** active
- **Summary:** Current `scripts/cairn_outbox.py` replay, including a version 1 queue, fails closed on 301, 302, 303, 307, and 308. It posts only `POST /cairn-memory/sync/push` and does not request the Location or forward the bearer. A redirect keeps the event pending with its payload unchanged and one added attempt. Only a 401 purges. Do not restore the old `urlopen` follow behavior for version 1 compatibility. A later queue version must fail closed the same way. The handler must own `http_error_302` directly. Parent processing puts a non-http Location into `lastError` before `redirect_request`. Close the response without reading it. The stored error and command output are `HTTP Error <code>: redirect blocked` and omit the Location URL, header, and response body.
- **Reason:** Python 3.9 `urlopen` followed 301, 302, and 303 for POST, changed the method to GET, and copied `Authorization` to another origin. Its scheme check also interpolated `abc://` query text into `HTTPError` before the outbox override ran.
- **Evidence:** `scripts/cairn_outbox.py`; `scripts/smoke_outbox.py`; `docs/contracts/sync-layer3.md`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/audits/2026-11-12-memory-capture.md`; `python3 -W error::ResourceWarning scripts/smoke_outbox.py` (7, 15.976s). Hook tests, Rust, and PostgreSQL were not run.

### 2026-11-12 — Redact human-readable metadata on new writes

- **Type:** lesson
- **Status:** active
- **Summary:** New sessions, observations, explicit memories, corrections, summaries, and promoted copies redact the `WorkContext` lists `files`, `symbols`, `commands`, `tests`, `services`, `handoffs`, and `runbooks` with `redact_sensitive_content`. Import also redacts `next_steps`, `unresolved_work`, and `promotion_reason`, then keeps the bundle sensitivity label. Capture, remember, and correction floor the label from the original content, concepts, and those lists. A session has no sensitivity field, so a secret that existed only at session start does not raise a later empty capture. Repository, branch, worktree, commit, and ids stay. Old rows are not migrated. A new outbox queue file redacts string items in those same seven lists, and `content`, before `save`. The Python redactor does not floor sensitivity. After that redaction the server classifies the redacted text and cannot see the original secret. The server label limit is unchanged. A missing or null context stays missing or null. Any other context must be a JSON object, and each present one of those lists must be a list of strings. Otherwise `normalize` raises `ValueError` before `save`. The message names the field and does not include the supplied value. A rejected enqueue does not create or change the queue file. Unknown keys and identity scalars are not validated. Older spool files are not rewritten. File sync adoption still copies a stored observation unchanged.
- **Reason:** A non-secret label on raw context or import metadata could be promoted and shared even after content redaction. Raising the label needs the original text, which a session row cannot store without a new field.
- **Evidence:** `crates/core/src/lib.rs`; `crates/storage/src/lib.rs`; `scripts/cairn_outbox.py`; `scripts/smoke_outbox.py`; `docs/contracts/lifecycle-v1.md`; `docs/contracts/sync-layer3.md`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/audits/2026-11-12-memory-capture.md`; `cargo test -p cairn-core --lib` (36); `python3 -W error::ResourceWarning scripts/smoke_outbox.py` (4). PostgreSQL tests were not run. The core 36 count is from the Rust metadata pass and was not rerun for the outbox change.

### 2026-11-12 — Scope file account capture duplicates by owner

- **Type:** lesson
- **Status:** active
- **Summary:** `capture_for_account` acknowledges a duplicate only when that account already owns the event. A foreign owner and an unowned existing event are validation errors and must not return the row or record a new owner. `capture_observation` stays a global file key.
- **Reason:** The file store has one observation per event id. Returning the existing row from the account path leaked another account's text and let a later account claim an anonymous row.
- **Evidence:** `crates/core/src/lib.rs`; `docs/adr/0008-self-hosted-account-authentication.md`; `docs/contracts/auth-layer3.md`; `docs/audits/2026-11-12-memory-capture.md`; `cargo test -p cairn-core --lib -- capture_for_account_duplicate_stays_with_the_owner`

### 2026-11-12 — Do not attach a body to a 204 proxy response

- **Type:** lesson
- **Status:** active
- **Summary:** The console logout proxy must return `new NextResponse(null, { status: 204 })` and clear `cairn_token` only after that response is constructed. An empty `ArrayBuffer` is still a body, and the Fetch constructor throws `Invalid response status code 204`.
- **Reason:** The Rust logout already returns 204 and revokes the session. A thrown proxy response became a browser 500 and left the cookie in place.
- **Evidence:** `apps/web/app/api/cairn/[...path]/route.ts`; `apps/web/lib/session.ts`; `./scripts/smoke_web.sh`

### 2026-11-03 — Allow clean-build time in HTTP smoke startup

- **Type:** constraint
- **Status:** active
- **Summary:** HTTP smoke readiness must allow a clean Cargo target to compile the server before polling gives up; REST and full-flow checks now allow 60 seconds.
- **Reason:** The previous 12-second window could fail during a valid first compilation even though the server became healthy afterward.
- **Evidence:** `scripts/smoke_rest.sh`; `scripts/smoke_full_flow.sh`; clean-target workspace and acceptance suite runs

### 2026-11-03 — Cairn owns the public REST namespace

- **Type:** decision
- **Status:** active
- **Summary:** Cairn's public REST, hook, smoke, viewer, and Layer 3 contract paths use `/cairn-memory/`; upstream `/agentmemory/` paths remain only in the upstream compatibility inventory.
- **Reason:** Cairn Memory is an independent rebuild inspired by AgentMemory, not a package/runtime dependency or a product endpoint namespace.
- **Evidence:** `crates/server/src/lib.rs`; `README.md`; `docs/contracts/agentmemory-baseline.md`; `cargo test --workspace`; local smoke suite

### 2026-11-03 — Layer 3 completion boundary

- **Type:** decision
- **Status:** active
- **Summary:** Layer 3 is complete for the personal cross-device gate: PostgreSQL parity includes context retrieval, corrections, export/import, lifecycle and sync diagnostics, metadata-only audit history, rate-limited auth attempts, and revocation-safe outbox purge.
- **Reason:** A trusted-device memory system needs recovery, governance, and deletion behavior at the same boundary as authenticated replay.
- **Evidence:** `crates/storage/src/lib.rs`; `migrations/0002_layer3_audit.sql`; `scripts/smoke_postgres.sh`; `scripts/smoke_layer3.sh`; `cargo test --workspace`

### 2026-11-11 — Layer 5 team space and retained delete

- **Type:** decision
- **Status:** active
- **Summary:** A team space belongs to one project. The only roles are owner and member. The owner creates it, names it, and adds or removes an existing project member. Removal drops that project membership and revokes active shares; a former member's shared read is empty and the memory stays stored. Delete is owner-only, uses the same tombstone as forget, keeps the record in storage and export, writes `memory_delete_retained`, and does not purge other devices. Offline purge, encryption, federation, and CRDTs stay unimplemented.
- **Reason:** Layer 5 can close without adding roles or physically deleting memory.
- **Evidence:** `docs/roadmap.md`; `docs/contracts/share-layer5.md`; `docs/adr/0011-explicit-project-memory-share.md`; `migrations/0010_layer5_team_space.sql`; `cargo test --workspace --all-targets`; `./scripts/smoke_layer5_cli.sh`; `./scripts/smoke_viewer.sh`; `./scripts/smoke_postgres.sh` on database `cairn_pg_layer5_team_delete`

### 2026-11-11 — Layer 5 policy, candidates, and visible conflicts

- **Type:** decision
- **Status:** active
- **Summary:** The default project share policy is `manual`, so promotion creates no share. `auto_nonsensitive_project_memory` shares a newly promoted project memory that is not secret and not sensitive with each current member and writes `project_memory_shared`. Share candidates are unshared promoted project memories; dismissing one does not share it, and accepting one uses the existing share method. Secrets and private observations never become shares. A conflict list keeps both contradictory memory ids. Turning the policy off does not revoke existing shares. The team space and retained delete are recorded in the later Layer 5 entry. Offline purge, encryption, federation, and CRDTs remain open.
- **Reason:** Automatic sharing has to fail closed unless the owner sets the one explicit policy the collaborative specification already allowed.
- **Evidence:** `docs/roadmap.md`; `docs/contracts/share-layer5.md`; `docs/adr/0011-explicit-project-memory-share.md`; `migrations/0009_layer5_share_policy.sql`; `cargo test --workspace --all-targets`; `./scripts/smoke_layer5_cli.sh`; `./scripts/smoke_viewer.sh`; `./scripts/smoke_postgres.sh` on database `cairn_pg_layer5_policy_smoke`

### 2026-11-11 — Explicit invite and share is the first Layer 5 slice

- **Type:** decision
- **Status:** active
- **Summary:** An owner invites an existing account by email and shares one already promoted project memory. The member can read that memory through `GET /cairn-memory/projects/{projectId}/shared`. The member cannot list the owner's project memories, search private observations, or correct or forget the shared memory. Revoke hides it from the member and leaves it stored for the owner. A live successor is omitted until the owner shares that successor. Audit actions are `project_member_invited`, `project_memory_shared`, and `project_memory_share_revoked`.
- **Reason:** Project memory must not become visible to another account through promotion alone, and a share must not copy the memory into personal recall or device sync.
- **Evidence:** `docs/adr/0011-explicit-project-memory-share.md`; `docs/contracts/share-layer5.md`; `migrations/0008_layer5_project_shares.sql`; `cargo test --workspace --all-targets`; ignored `crates/storage/tests/postgres.rs`; `./scripts/smoke_postgres.sh`

### 2026-11-03 — Explicit project promotion is the first Layer 4 slice

- **Type:** decision
- **Status:** active
- **Summary:** Project registration and `POST /cairn-memory/promote` create an explicit project-scoped decision while preserving source evidence and a redacted reason; branch/worktree/commit context is cleared, secret/forgotten/expired/repository-mismatched sources fail closed, repeated promotion is idempotent, and explicit merge/close transitions resolve registered branches and worktrees.
- **Reason:** Project memory must not broaden automatically from branch relevance; the owner must authorize the scope transition and be able to inspect why it happened.
- **Evidence:** `docs/adr/0009-explicit-project-promotion.md`; `docs/contracts/project-layer4.md`; `crates/core/src/lib.rs`; `crates/storage/src/lib.rs`; `migrations/0003_layer4_projects.sql`; `migrations/0004_layer4_freshness.sql`; `migrations/0005_layer4_project_entities.sql`; `migrations/0006_layer4_branch_resolution.sql`; project promotion/freshness/entity/merge tests

### 2026-11-03 — Rust idiom rule is repository-enforced

- **Type:** workflow rule
- **Status:** active
- **Summary:** Rust changes must follow the self-contained `RUST_IDIOMS.md` rule, which covers ownership, borrowing, error handling, API design, state transitions, async/database safety, privacy, documentation, rustfmt, and Clippy verification. OpenCode loads it automatically with `MEMORY.md`.
- **Reason:** The project needs one durable, agent-visible Rust standard rather than relying on an external reading list or inconsistent local style decisions.
- **Evidence:** `RUST_IDIOMS.md`; `AGENTS.md`; `opencode.jsonc`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`

### 2026-11-12 — Hook transport uses one explicit credential

- **Type:** decision
- **Status:** superseded
- **Summary:** `scripts/cairn_hook.py` posts `/cairn-memory/observe` with no bearer when neither `CAIRN_ACCOUNT_TOKEN` nor `CAIRN_DEVICE_TOKEN` is set. An account token is sent only to observe. A device token is sent only to `/cairn-memory/sync/push`, including PostgreSQL, which has no observe route. Both set, or a value that is not one header token, exits before stdin is read and before any request. There is no generic token, server detection, auth-error fallback, or redirect. The device body is redacted with `cairn_outbox.normalize` before the request. Observe is not client-redacted, so the file server still classifies the original content. The fingerprint stays session, source, and content. The hook does not start or end a session or register a device. Tokens are environment values, not arguments, and are not stored in the payload. The "does not enqueue" sentence is superseded by the device-only `CAIRN_OUTBOX` entry below. Unset `CAIRN_OUTBOX` still drops a failed delivery.
- **Reason:** Account and device tokens are different verifiers. One shared token, or a retry on the other route, would cross the approved boundary. PostgreSQL capture stays on device sync.
- **Evidence:** `scripts/cairn_hook.py`; `scripts/smoke_hook_auth.py`; `docs/contracts/hook-v1.md`; `docs/contracts/auth-layer3.md`; `docs/contracts/sync-layer3.md`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/adr/0008-self-hosted-account-authentication.md`; `docs/audits/2026-11-12-memory-capture.md`; `python3 scripts/smoke_hook_auth.py StubHookTests` (13 ok); `python3 scripts/smoke_hook_auth.py FileServerHookTests` (1 ok, temp file store); `./scripts/smoke_hook.sh`. PostgreSQL HTTP was not run. The enqueue sentence is superseded by the device offline entry.

### 2026-11-12 — Version 2 outbox is device-bound

- **Type:** decision
- **Status:** active
- **Summary:** `enqueue_bound` creates a version 2 queue only when the file is missing. The file stores `sync/push`, a normalized server origin, and SHA-256 of `cairn-outbox-v2`, a NUL, and the raw device token. It does not store the token. A hostname containing `%` is rejected with the fixed origin message and is not decoded. IPv6 zone ids are unsupported. Bracketed `[::1]` and `localhost` stay valid. Version 1 enqueue refuses that file and does not convert it. Version 2 replay checks origin, route, and fingerprint before any socket. A mismatch leaves the bytes unchanged and does not purge. A matching 401 still purges. `CAIRN_ACCOUNT_TOKEN` is not a bearer. Version 2 rejects a `--token` that is exactly that value before a socket. Version 1 replay does not read it and does not use this normalizer. One file is one token and one origin. There is no rebind command. A new token fails closed and leaves the queue. The old token still matches, so its 401 still purges. Manual recovery is a new file. Same-thread lock reentry is the lock inode, not the path spelling. The hook appends a retryable device failure only when `CAIRN_OUTBOX` is set, and only with `LOCK_NB`, so it does not wait while replay holds this lock across a socket call. See the device offline entry below.
- **Reason:** An unbound spool can replay a device capture with the wrong token or to the wrong server. `urllib` unquotes `%2e` and `%3a` in the host after the queue has stored the encoded form, so the bearer would go to a different host or port. A path-keyed reentry lock deadlocks when `/tmp` and `/private/tmp` name one inode. Rebind would let a local command change the fingerprint without proving it is the same device.
- **Evidence:** `scripts/cairn_outbox.py`; `scripts/smoke_outbox.py`; `docs/contracts/sync-layer3.md`; `docs/contracts/hook-v1.md`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/audits/2026-11-12-memory-capture.md`; `python3 -W error::ResourceWarning scripts/smoke_outbox.py` (21 tests, 29.247s, exit 0); `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py`. Hook tests, Rust, and PostgreSQL were not run. The changes are uncommitted on Andress-MacBook-Air. `version: true` is still version 1. Unicode host policy was not changed.

### 2026-11-12 — Device hook offline enqueue is opt-in

- **Type:** decision
- **Status:** active
- **Summary:** `CAIRN_OUTBOX` queues only a retryable `CAIRN_DEVICE_TOKEN` hook failure, once, into the existing version 2 file. Unset still posts once and drops. Observe and account deliveries are not queued, and their failure text does not say the event was retained. The hook uses the canonical origin for the request and the queue, rejects a `%` host, userinfo, query, fragment, or path before a socket, and checks an existing queue with `LOCK_NB` before the POST. The append checks the binding again and does not hold the lock during the network call. Exit 0 happens only after the save or when the same event id is already in that file. A busy lock, a full queue, or a disk error exits 1, keeps the previous bytes, and does not retain the capture. A caller that ignores the nonzero exit can drop it. There is no daemon and no second POST. Replay stays explicit, does not read `CAIRN_DEVICE_TOKEN`, and purges a 401 only after the version 2 identity matches. An empty context inherits the person's session when replay is accepted, not at hook time. There is no rebind command. A rotated token mismatches. Raw identity fields and old spool bytes stay unsupported.
- **Reason:** PostgreSQL capture is already device `sync/push`, and the version 2 queue already replays only that route. Queuing observe into that file would replay it as another device's push. A blocking lock would stall the hook while replay holds the lock across a socket.
- **Evidence:** `scripts/cairn_hook.py`; `scripts/cairn_outbox.py`; `scripts/smoke_hook_offline.py`; `docs/contracts/hook-v1.md`; `docs/contracts/sync-layer3.md`; `docs/adr/0007-layer3-local-trusted-device-slice.md`; `docs/audits/2026-11-12-memory-capture.md`; `python3 -W error::ResourceWarning scripts/smoke_hook_offline.py` (11 tests, 24.544s, exit 0); `python3 -W error::ResourceWarning scripts/smoke_outbox.py` (21 tests, 28.128s, exit 0); `python3 -W error::ResourceWarning scripts/smoke_hook_auth.py StubHookTests` (13 tests, 2.197s, exit 0); `FileServerHookTests` (1 test, 40.068s, exit 0); `./scripts/smoke_hook.sh` passed. Rust, PostgreSQL, Linux, and Windows were not run. Uncommitted on Andress-MacBook-Air. Python 3.9.6.

### 2026-11-13 — File account search keeps owned matches before the caller limit

- **Type:** lesson
- **Status:** active
- **Summary:** `retain_account_recall` keeps `memory_owners` rows for that person, drops contradiction ids the person does not own, and sets `contradictory` from what remains. `search_for_account` calls it after an unrestricted eligible scan and before the caller limit. `resume_for_account` and `unresolved_for_account` authenticate, scan with `usize::MAX`, call it, then pack. Context and why-changed go through search. A resume pack of `ownerfact` and `memberfact` does not prove this, because those texts do not contradict. Unscoped search and unscoped resume still return foreign ids. PostgreSQL search puts `person_id` in `WHERE` before `LIMIT`. CLI and MCP stay on unscoped `search_with_filter`. This does not close the rest of account retrieval.
- **Reason:** Applying the caller limit before ownership hides an owned match, and `recall_result` can name another person's memory until the account retain runs.
- **Evidence:** `crates/core/src/lib.rs`; `docs/audits/2026-11-13-account-search-limit.md`; `cargo test -p cairn-core --lib` (38 passed, 2.41s); `cargo test -p cairn-server --lib -- file_http_capture_uses_file_bearer_when_accounts_exist` (1 passed, 0.64s); `cargo clippy -p cairn-core --all-targets -- -D warnings`. Uncommitted on Andress-MacBook-Air. Workspace tests, PostgreSQL, and live ports were not run. Global filter inheritance and contradiction-target eligibility are `docs/audits/2026-11-13-recall-filter-eligibility.md`, not this limit fix.

### 2026-11-13 — Active contradiction targets are recall-eligible

- **Type:** decision
- **Status:** active
- **Summary:** Choice B. `contradictory` means a recallable same-scope memory still competes. `contradiction_ids` skips a target forgotten by memory id or source event, expired at recall time, or superseded by a live successor, using the same checks as `memory_is_recalled`. `supersedes` and export keep the predecessor. Forgetting or expiring the successor makes the predecessor eligible again. There is no automatic winner and no new response field. A caller limit does not drop an owned live peer id. Account ownership filtering still runs after eligibility. PostgreSQL already passes an eligible person slice into `recall_result`; the helper does not need full history and does not change that query.
- **Reason:** A stale row is not an active competitor. The predecessor stays provenance through `supersedes` and export, including while the correction is the only live claim.
- **Evidence:** `crates/core/src/lib.rs`; `docs/contracts/continuity-v1.md`; `docs/adr/0010-correction-keeps-linked-history.md`; `docs/audits/2026-11-13-recall-filter-eligibility.md`; `cargo test -p cairn-core --lib` (39 passed, 7.58s); `cargo clippy -p cairn-core --all-targets -- -D warnings`; `cargo test -p cairn-server --lib -- why_changed_route_returns_reason_relationships_and_conflicts`; `cargo test -p cairn-server --lib -- correction_replaces_recall_without_destroying_provenance -- --exact`. PostgreSQL was not rerun. Uncommitted on Andress-MacBook-Air.

### 2026-11-13 — Global recall inheritance is contractual

- **Type:** lesson
- **Status:** active
- **Summary:** A `global` memory matches every session, repository, branch, and worktree filter. A project memory matches that repository's narrower filters. Branch and worktree memories do not match a different branch, worktree, or repository. Same-public-session account resume still omits the other person. Superseded 2026-11-13 for contradiction targets: the file store no longer cites a superseded, pre-sweep expired, or source-event-forgotten row. See the active-target entry.
- **Reason:** The global match is the Layer 2 rule that personal guidance stays available. Treating it as cross-project contamination would fight the contract. The contradiction-target half of this entry is the active-target entry.
- **Evidence:** `crates/core/src/lib.rs`; `crates/storage/src/lib.rs`; `docs/contracts/continuity-v1.md`; `docs/adr/0010-correction-keeps-linked-history.md`; `docs/audits/2026-11-13-recall-filter-eligibility.md`; `cargo test -p cairn-core --lib -- scoped_recall_keeps_branch_memory_isolated -- --exact`; `/tmp/cairn-audit-010-harness`; disposable database `cairn_audit010_20261113` on container `cairn-pg-verify-4963`, then dropped. The later contradiction-target code change is the active-target entry.

## Open questions worth preserving

Add only questions that remain relevant across sessions. Remove or update them
when an ADR, specification, or implementation resolves them.

### 2026-11-13 — PostgreSQL context counts all matches and why-changed ranks before the 512 cut

- **Type:** decision
- **Status:** active
- **Summary:** PostgreSQL keeps a 512-match search window. `context` and `why_changed_context` set `omitted_count` to all eligible matches minus returned rows (`COUNT(*) OVER ()`), and `omitted_memory_ids` stays inside the window. `why_changed_context` ranks every eligible match with `why_changed_priority` (decision, then relationship count, then lexical order) before the cut, so its work grows with the number of matches. Plain `search` stays lexical.
- **Reason:** A cap applied before counting or ranking hid omitted matches and a rank-513 decision.
- **Evidence:** `crates/storage/src/lib.rs`; `crates/core/src/lib.rs`; `docs/audits/2026-11-13-postgres-search-cap.md`; `crates/storage/tests/postgres.rs` ignored tests `postgres_context_omissions_count_every_eligible_match_past_the_cap` and `postgres_why_changed_orders_like_the_file_store`

### 2026-11-13 — A foreign `supersedes` link is stored inert in the file store

- **Type:** decision
- **Status:** active
- **Summary:** In the file store a live successor hides its target in recall and in contradiction targets unless the successor has an owner that differs from the target's owner. An unowned successor (local CLI and MCP, pre-account data) still counts. Import does not reject the link, and export keeps it unchanged. `superseded_targets` builds the set once per request; `recall_result` keeps its signature. PostgreSQL already compares `person_id`. Open: account import still accepts foreign `forgottenIds` (CM-AUDIT-012 finding 2).
- **Reason:** One account's import must not hide another account's memory, and a link to an unknown id must stay importable and stay inert if the target later appears.
- **Evidence:** `crates/core/src/lib.rs`; `docs/audits/2026-11-13-file-supersedes-ownership.md`; `docs/adr/0010-correction-keeps-linked-history.md`; `cargo test -p cairn-core --lib` (43); `cargo test -p cairn-server --lib` (30)

### 2026-11-13 — File account tombstones, forget and source cascades require ownership

- **Type:** decision
- **Status:** active
- **Summary:** `import_for_account` applies a `forgottenIds` entry only when the caller owns the id or the same bundle carries a memory or observation row with that id. An id owned by another person rejects the whole import with the row-ownership error and writes nothing. Every other id, including a source event id with no row, is ignored and not counted. A row's `source_event_ids` are not proof. Unscoped import, local CLI, MCP and a no-account store keep every entry. PostgreSQL tombstones are person-keyed and unchanged. Old stored tombstones are not repaired. Account forget and delete follow the same rule (independently accepted 2026-11-13, file store): the caller must own the id (an unowned legacy, foreign or unknown id reads `unknown memory or event`), a forget cascades only source ids the caller owns, and `remember_for_account` owns the source id it generates (an owner record with no observation row, in the same write as the memory); source ids named in row metadata are never authority. `id_owned_by_other` checks memory and observation owner records so capture, device push and import cannot take such an id. No backfill: old unowned generated sources stay unclaimed and can still be captured by an account that knows the id. Trusted unscoped forget keeps its full cascade. Open: old-data repair and id squatting.
- **Reason:** File ids are globally unique and the first owner keeps them, so only an owner should be able to forget an id. A foreign or future id named as a source would otherwise poison another account. Memory ids and event ids share one id space: `capture_for_account` and device `sync_push` reject an event id that another person's memory row owns (`event belongs to another person`), so a tombstoned squat fails loudly. The foreign-id error reveals that an id is owned; first-owner id reservation by an importer is not prevented.
- **Evidence:** `crates/core/src/lib.rs`; `docs/audits/2026-11-13-file-import-tombstone-ownership.md`; `docs/contracts/export-v1.md`; `docs/audits/2026-11-13-file-import-tombstone-ownership.md` (findings 2 and 3); `docs/contracts/auth-layer3.md`; `cargo test -p cairn-core --lib` (54); `cargo test -p cairn-server --lib` (33)

### 2026-11-13 — File-store project access is owner-only through a strict owned-project check

- **Type:** decision
- **Status:** active
- **Summary:** In the file store `project_maintenance`, branch and worktree registration and listing, entity registration and listing, freshness history, `confirm_branch_merge` and `close_branch` call `require_owned_project` after authentication: the project must exist and have an owner record equal to the caller, otherwise `account does not own the project` (members, removed members, outsiders and ownerless projects are refused; members use explicit sharing). Merge also rejects another account's source memory, and its existing-promotion and merged-branch lookups use `project_rows` (owner-visible rows only). Not repaired: old aliased branches are denied the read, an ownerless project is claimed by the first account that registers its repository, other functions still use the lenient `require_project_owner`, and foreign vs missing project errors differ. PostgreSQL is not covered.
- **Reason:** The project id is not a secret and an authenticate-only check let any account read or write another's registry and copy its memory. The lenient helper passes any account for an ownerless project, so strict ownership needs its own helper.
- **Evidence:** `docs/contracts/project-layer4.md`; `docs/audits/2026-11-13-file-import-project-id.md`; `docs/audits/2026-11-13-project-registry-access.md`; `crates/core/src/lib.rs`

## Entry template

```md
### YYYY-MM-DD — Short title

- **Type:** decision | constraint | lesson | correction
- **Status:** active | superseded
- **Summary:** What is true or what was decided.
- **Reason:** Why it matters.
- **Evidence:** `path/to/file`, command, test, or commit
```
