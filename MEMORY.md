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

### PostgreSQL is the server-authoritative Layer 3 sync path

- **Type:** decision
- **Status:** active
- **Summary:** `cairn-storage` provides the async PostgreSQL account, device, observation, sync-event, cursor, session-summary, explicit-memory, forget, and keyword-search repository; `cairn-server --database-url` runs that boundary and applies the initial migration.
- **Reason:** The file-backed adapter remains useful for local Layer 1/2 compatibility, while canonical cross-device writes need transactional server persistence.
- **Evidence:** `crates/storage/src/lib.rs`; `migrations/0001_layer3_personal_memory.sql`; `crates/server/src/main.rs`; `crates/storage/tests/postgres.rs`; `scripts/smoke_postgres.sh`; PostgreSQL 16 in Colima integration run

## Lessons and recurring corrections

Add only durable lessons here. Corrections to the agent’s working approach
should include the reason they matter and the evidence that supports them.

### 2026-11-03 — Scope sync duplicate checks by person

- **Type:** lesson
- **Status:** active
- **Summary:** A sync event ID is not globally safe for duplicate acknowledgement. A duplicate must first resolve to a persisted sync event owned by the authenticated person; collisions outside that scope must be rejected.
- **Reason:** A global observation lookup previously allowed another person’s device to receive the existing observation when it reused the same event ID.
- **Evidence:** `crates/core/src/lib.rs`; `crates/server/src/lib.rs`; `cargo test --workspace`; `./scripts/smoke_layer3.sh`

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

## Open questions worth preserving

Add only questions that remain relevant across sessions. Remove or update them
when an ADR, specification, or implementation resolves them.

## Entry template

```md
### YYYY-MM-DD — Short title

- **Type:** decision | constraint | lesson | correction
- **Status:** active | superseded
- **Summary:** What is true or what was decided.
- **Reason:** Why it matters.
- **Evidence:** `path/to/file`, command, test, or commit
```
