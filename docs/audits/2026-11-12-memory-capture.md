---
id: CM-AUDIT-008
title: "Memory capture and persistence — 2026-11-12"
status: draft
---

# Memory capture and persistence — 2026-11-12

Capture has two different doors. The file store accepts a local session, an observation, and a session summary. PostgreSQL accepts the same session calls only from an account, and it accepts observations only from a device sync push. This audit follows that write path. It does not judge ranking, retrieval quality, corrections, forgetting, or the share protocol.

## What actually happens

A local capture starts with `POST /cairn-memory/session/start`, `scripts/cairn_hook.py`, the `cairn capture` CLI, or MCP `memory_observe`. The hook fingerprints `sessionId + source + content` when no event id is present. With neither credential set, it posts JSON to `/cairn-memory/observe` with no bearer token. `CAIRN_ACCOUNT_TOKEN` alone adds that account bearer on observe. `CAIRN_DEVICE_TOKEN` alone posts `/cairn-memory/sync/push` with that device bearer. Both set does not post. With `CAIRN_OUTBOX` unset, a down server still drops the event. With that path set, only a retryable `CAIRN_DEVICE_TOKEN` failure is queued. Observe and account failures are not queued. The server checks that the ids, role, source, and content are non-empty and that the body is at most 64 KiB. It redacts four markers (`authorization: bearer `, `api_key=`, `token=`, `password=`), a case-insensitive `sk-` token, and recognized private-key PEM blocks in content and in the human-readable `WorkContext` lists (`files`, `symbols`, `commands`, `tests`, `services`, `handoffs`, `runbooks`). It classifies sensitivity from the original content and those lists, keeps a caller label only when it is at least that strict, and appends one observation. Repository, branch, worktree, and commit are stored as sent. The same `event_id` is an idempotency key for the whole file in `capture_observation`. A later unscoped call returns the first observation and discards the new text. `capture_for_account` returns that stored observation only when the authenticated account already owns the event. Once any account exists, file HTTP start, observe, end, and remember require `file_bearer` and do not call the unscoped methods. A store with no accounts still does.

If the observation's context is entirely empty and that caller's session row exists, the observation copies that row. An account copies only its `(person_id, session_id)` row. An unscoped capture copies only the row with no `person_id`. A missing row leaves the context empty. Any non-empty context, even a single file, is kept instead of the session row. Session start redacts the human-readable lists before the row is stored. The session has no sensitivity field, so a secret that existed only there does not raise a later empty capture's label. A non-empty capture context is redacted and classified from its original lists. On a file store with accounts, `POST /cairn-memory/session/end` calls `end_session_for_account` and builds `memory-{person_id}-{session_id}` from that account's retained observations. A store with no accounts still calls `end_session` and builds `memory-{session_id}` from unscoped observations in the session. Explicit `remember` creates a separate durable memory with a new nanosecond id and no idempotency key. With an account it records that account as owner. With no accounts it stays unscoped.

Account-scoped library methods exist. `capture_for_account` records the owner of a new event id. A retry by that owner returns the stored observation. An event owned by another account, or an existing event with no owner, is a validation error: the row is not returned and is not claimed. `end_session_for_account` builds `memory-{person_id}-{session_id}` from that person's events only. File HTTP uses those methods when `file_bearer` returns a token. When any account exists, a missing bearer is 401 and does not fall through to the global key. Start calls `start_session_for_account`. The account row is `(person_id, session_id)`.

PostgreSQL keys sessions and observations by `(person_id, event_id)` or `(person_id, session_id)`. There is no `/cairn-memory/observe` route. That is the approved capture split: an account token starts and ends a session, and a device token pushes through `/cairn-memory/sync/push`. An empty push copies this person's existing session context before normalize. A partial context stays as sent. If no session exists, the push inserts an empty one for the observation foreign key. A later account start fills a still-empty session with the redacted request. The first non-empty context stays. Observations and sync payloads are not rewritten. Session end applies the session context to in-memory copies of observations that are still empty, and only while building `memory-{person_id}-{session_id}`. A duplicate event id for the same person returns the stored row. The same event id for another person is a different row. An imported observation with no sync event stays outside sync.

The local outbox is a separate program. Before it writes a new queue file, it redacts `content` and string items in the human-readable `WorkContext` lists (`files`, `symbols`, `commands`, `tests`, `services`, `handoffs`, `runbooks`) with the four markers, a case-insensitive `sk-` token, and the five recognized private-key PEM labels. Repository, branch, worktree, commit, ids, dates, and cursors stay. A missing or null context stays missing or null. Any other context must be a JSON object, and each present one of those lists must be a list of strings. A bad shape raises `ValueError` before a new queue file is created or an existing file is changed. The error names the field and does not include the supplied value. Unknown keys and identity scalars are not validated. It queues up to 256 events and replays them to `/cairn-memory/sync/push`. It does not rewrite an older spool and does not apply the sensitivity floor. `enqueue`, `replay`, and `purge` hold one exclusive `fcntl.flock` on the sibling `<path>.lock` for the whole load, modify, and replace. `save` writes a unique temporary file in that directory and replaces the queue once. If locking is unavailable the command does not write. Version 1 stays unbound. Version 2 is created only by `enqueue_bound` when the file is missing, and replay checks the origin, route, and device fingerprint before any socket. Same-thread lock reentry follows the lock inode. The hook calls `enqueue_bound` only for an opt-in retryable device failure, using `LOCK_NB`, and it does not replay.

## Findings

### 1. File HTTP capture ignores the account boundary — high, confidence high

Paths: `crates/server/src/lib.rs:2200-2232` and `:2387-2414`. Library behavior: `crates/core/src/lib.rs:3407-3479` and `:3931-3943`.

Search, export, and correct use `file_bearer`, which requires an account once any account exists. Session start, observe, session end, and remember originally did not. Session end always called `end_session`. MCP `memory_observe` and `cairn capture` / `cairn end` call the same unscoped functions (`crates/server/src/mcp.rs:278-297`, `crates/core/src/bin/cairn.rs:101-128`). That is a reasonable single-user CLI. It was the wrong HTTP boundary for a store that has accounts.

Trigger: two accounts capture different text into session `shared-session`, then `POST /cairn-memory/session/end` runs. The same thing happens if the hook posts into a session an account already used.

Impact: one summary, `memory-shared-session`, contains both accounts' text. It has no owner. `search_for_account` drops it, so the owner cannot recall the summary the HTTP end just wrote. The other account's text is in that summary.

Minimal fix: when `has_accounts()` is true, require a bearer and call the account-scoped capture, end, and remember. Authenticate start, then call the existing `start_session`. Keep the unscoped functions for a store with no accounts.

Check: two account tokens plus one anonymous observe against a temporary file server, then session end and each account's search. Isolated harness result before the fix: `PASS session end mixes accounts and account search misses it`.

**HTTP status:** fixed in the four file handlers. With accounts, missing and invalid bearers return 401 and do not write. Observe calls `capture_for_account`, end calls `end_session_for_account`, and remember calls `remember_for_account`. Start calls `start_session_for_account`. Zero-account HTTP stays unscoped. CLI and MCP stay unscoped. The default hook still sends no bearer, so after bootstrap that call receives 401. There is no anonymous fallback. An explicit account token on observe is the later hook transport, not a bypass of this gate. Findings 3–6 were not changed by this gate. Finding 7 credential transport is recorded below. Shared session context is choice B below, not one global row.

### 2. A repeated file event id returns another account's observation — high, confidence high

Path: `crates/core/src/lib.rs:3334-3390`. The sync path rejects this case at `:3209-3231`. PostgreSQL does not share the key (`migrations/0001_layer3_personal_memory.sql:57-69`).

Trigger: account A stores event `shared-event`. Account B posts the same event id with different text, with a valid bearer.

Impact: B receives `duplicate: true` and A's stored text. B's text is not stored. `record_observation_owner` (`:2659-2671`) keeps the first owner, so this is a read leak and a lost write, not an ownership transfer. If the first write was anonymous, the later account becomes the owner of the anonymous text.

Minimal fix: resolve a duplicate only inside the caller's observations. A foreign event id is an error, as sync already does.

Check: the existing test covers sync, not capture. Harness result before the fix: `PASS cross-account event id returns the other person's text`.

**Library status:** fixed in `capture_for_account`. Authentication runs before the event id is examined. An owned retry still returns `duplicate: true` and that observation. A foreign id uses `event belongs to another person: {event_id}`. An unowned id uses `event already exists outside sync scope: {event_id}`. Neither error includes the stored observation or writes an owner. The file still stores one observation per event id. CLI, MCP, sync collision checks, and zero-account HTTP stay on their previous keys. Finding 3 later rolls a failed persist back; it does not change these duplicate ownership rules. File HTTP observe without a bearer is 401 once any account exists, so that route no longer returns the other row.

### 3. A failed file write is acknowledged on retry — high, confidence high

Path: `crates/core/src/lib.rs:3366-3367` and `:4127-4135`.

The observation is pushed into memory and then written. If the rename fails, the error is returned but the push stays. The retry finds that in-memory row, returns `duplicate: true`, and does not write again. A restart loads the last good file, and the event is gone. The client was told it was stored.

`sync_push` writes twice: `capture_observation` persists, then the sync event is persisted (`:3241-3251`). If the second write fails, the observation is on disk without a sync event. The retry hits finding 4.

Minimal fix: write first, or remove the pushed row when `persist` returns an error. Acknowledge a duplicate only from the last successful state.

Check: harness made the store directory read-only after one successful capture. Result: `PASS failed persist then retry reports a duplicate that disk does not have`.

**Library status:** fixed for the three file capture commits. `capture_observation` stages the observation and publishes it with one rename. `capture_for_account` stages that observation and the owner record, then uses the same single rename. `sync_push` stages the observation and its sync event, then uses one rename; the device last-seen change for that new push is in the same rename. If the rename fails, the staged rows are removed and the call returns the IO error. A retry writes. It does not return `duplicate: true` for data the file does not contain. Reopen matches that file. An earlier successful observation stays. An owned account duplicate and an unscoped duplicate return the stored row and do not write the file again. A sync duplicate still persists the device last-seen update and does not add another observation or sync event. A failed new sync push does not leave an observation without its sync event. An observation that already exists outside sync is still rejected by this fix. Another person's event id is still rejected. The file format is unchanged, and the rename is still not followed by `fsync`. Finding 4 later attaches one sync event when the persisted owner is the pushing person.

Before the fix, the three new library tests failed because the observation remained in memory after the IO error. All three tests now block persistence the same way: replace the store directory with a file so the parent create fails. That is a parent-directory persistence failure.

### 4. A hooked observation cannot later sync under the same event id — medium, confidence high

Path: `crates/core/src/lib.rs:3209-3225`. Outbox replay posts that same id to `/sync/push` (`scripts/cairn_outbox.py:82-98`).

Trigger: `/observe` or `cairn capture` stores `evt-1`. A device then pushes `evt-1`.

Impact: the push returns `event already exists outside sync scope`. The outbox keeps the event and retries. Pull returns nothing, so another device never sees a capture that is already in the file. A failed sync rename no longer leaves that observation. Observe-then-sync of the same event id is still this finding.

Minimal fix: if the event is already owned by the pushing person and has no sync row, append the sync row. Keep the foreign-person rejection.

Check: harness result: `PASS observe-then-sync of the same event id is stuck outside sync`. Existing `sync_event_id_collision_cannot_leak_another_persons_observation` passed and still rejects the other person's id.

**Library status:** fixed on the file adapter. `sync_push` attaches one sync event when the observation has no sync event and `observation_owners` shows the authenticated device's person owns it. The sync event copies the stored observation. The request content, session, context, and any person text are not consulted as ownership and do not replace the row. The first attachment returns `duplicate: false` and a new cursor, because that flag already means "this person's sync event is already stored." The next push returns `duplicate: true` and the same cursor. A pull for that person returns the event once. A failed rename drops the new sync event, cursor, and device last-seen change and leaves the observation and owner record. A retry then attaches the event. An observation with no owner still returns `event already exists outside sync scope` and is not claimed. An observation owned by someone else, and another person's sync event, still return `event belongs to another person` with no observation body. A new sync push still commits its observation and sync event together, and an empty context now copies the device person's session row rather than the unscoped row. The file still has one observation per event id. PostgreSQL, redaction, the hook, and stored legacy context are unchanged. Unscoped CLI and MCP captures still have no owner, so they stay outside sync.

### 5. Private keys and uppercase `SK-` values are stored — medium, confidence high

Paths: `crates/core/src/lib.rs` `redact_sensitive_content` and `floor_sensitivity`. Outbox: `scripts/cairn_outbox.py` `redact_content`, called from `normalize` before `save`. Policy requires redaction of secrets, bearer tokens, and private keys before sync (`docs/policy.md:28`). The hook contract leaves redaction to the server (`docs/contracts/hook-v1.md:38-39`).

Original observation: the four markers were redacted case-insensitively. `sk-` was redacted only when those exact letters appeared, so `SK-live-secret-value` was stored. The classifier still labeled that text secret because it lowercases the scan. A PEM block was not a marker, so `SECRETKEYMATERIAL` between the PEM lines was stored and was not labeled secret unless another marker was present. The outbox redacted `token=` and left `SK-` in the spool until replay.

Caller-supplied `lifecycle.sensitivity` replaces the classifier (`crates/core/src/lib.rs:107-110`). Promotion refuses only `secret` (`:1764-1767`). Auto-share refuses `secret` and `sensitive` (`:3006-3012`). A public label on text the classifier missed can pass those gates. Sharing itself was not exercised.

Minimal fix: redact PEM blocks and a case-insensitive `sk-` in both the server and the outbox. Store the higher of the caller label and the classifier.

Original check: harness result: `PASS SK- and a private key block stay in the stored observation` (value retained, label secret because of `SK-`). Outbox check: `token=` removed, `SK-live-outbox-secret` still in the queue file. Existing `redacts_sensitive_values_before_persistence` passed for `Authorization: Bearer` and `api_key=`.

**Content redaction status:** done for core ingress and for new outbox `content`. `redact_sensitive_content` and `floor_sensitivity` in `crates/core/src/lib.rs` are the shared server path. File capture, file sync, PostgreSQL `sync_push`, explicit remember, and correction all use them. A case-insensitive `sk-` token and a recognized private-key PEM block (`PRIVATE KEY`, `RSA PRIVATE KEY`, `EC PRIVATE KEY`, `OPENSSH PRIVATE KEY`, `ENCRYPTED PRIVATE KEY`) are removed before persistence. Classification uses the original text. The stored sensitivity is the stricter of the caller label and that detection, so `public` cannot lower a detected secret, and a stricter caller label stays. Ordinary prose, a public certificate, a public key, UTF-8 text, and the words `PRIVATE KEY` without a private-key header stay. A header without `END` drops that header line and following base64 or OpenSSL `Proc-Type` / `DEK-Info` lines. A line of optional PEM base64 glued to `-----END <label>` without the closing `-----` drops those key bytes and that footer token; text after the token stays. A `BEGIN` that never closes with `-----`, and labels outside the five, stay. Import redacts observation and memory content and still keeps the bundle's sensitivity label. Already stored files are not rewritten. `scripts/cairn_outbox.py` applies those same content patterns before it writes a new queue file. It does not rewrite an older spool and does not apply the sensitivity floor. Context-list coverage is the metadata status below. This is not a general secret scanner and it is not a privacy-safe declaration for capture. Findings 6 and 7 were not changed.

**Metadata redaction status, Rust:** done for new core writes on 2026-11-12. `redact_work_context` and `redact_memory_metadata` in `crates/core/src/lib.rs` run `redact_sensitive_content` over `files`, `symbols`, `commands`, `tests`, `services`, `handoffs`, and `runbooks` before a new session, observation, explicit memory, correction, derived summary, or promoted copy is stored. Import also redacts memory `next_steps`, `unresolved_work`, and `promotion_reason`. Capture, explicit remember, and correction classify the original content, concepts, and those lists, and they keep a stricter caller label. Import keeps the bundle label after redacting those strings. A kept `public` label does not mean the original metadata was free of secrets. A session has no sensitivity field: start redacts the lists, and an empty capture classifies the stored text, so a secret that existed only on the session does not raise the observation label. Repository, branch, worktree, commit, ids, timestamps, and provenance links are not redacted. A supported secret in one of those fields stays. PostgreSQL session start calls `redact_work_context`. PostgreSQL remember, correction, sync, import, and summary derivation use the same core builders. Already stored rows are not rewritten. A new correction or promotion redacts the metadata it copies onto the new row and leaves the source row unchanged.

**Metadata redaction status, new outbox writes:** done for string items in the seven `WorkContext` lists. `normalize` in `scripts/cairn_outbox.py` runs `redact_content` on those items before `save`. Repository, branch, worktree, commit, ids, dates, and cursors stay. The Python redactor does not floor sensitivity. Server `normalize_observation` still does, and after this client redacts it classifies the redacted text, so it cannot see the original secret. The documented label limit is unchanged. A missing or null `context` stays missing or null. Any other context must be a JSON object, and each present one of those lists must be a list of strings, matching `WorkContext`'s `Vec<String>` fields (`null` is not a vector). Otherwise `normalize` raises `ValueError` before `save`. The message is `outbox context must be a JSON object` or `outbox context <field> must be a list of strings`, and it does not include the supplied value. A rejected enqueue leaves a missing file missing and leaves an existing queue file byte-for-byte unchanged, so a kept event is not dropped. Unknown keys are copied and are not validated. Identity scalars are not redacted. This is not a recursive scrubber. The outbox does not send import `next_steps`, `unresolved_work`, or `promotion_reason`. Old spool files are not rewritten. Old stored metadata is not migrated. Do not treat a previously stored raw context string as cleaned.

### 6. PostgreSQL empty session context — fixed for the tested ordering; no observe route by policy

The missing `POST /cairn-memory/observe` route is the approved account-versus-device split, not a missing handler. File observe stays at `crates/server/src/lib.rs:561`. PostgreSQL session start and end stay at `:700-701`, and device push stays at `:723`. PostgreSQL capture stays on device `sync/push`. The hook's default is still unauthenticated observe. Explicit device delivery is finding 7. The credential boundary is implemented. Device offline enqueue is the verified `CAIRN_OUTBOX` opt-in. Observe offline enqueue is out of scope.

`PostgresStore::sync_push` copies this person's stored session onto an exactly empty push before `normalize_observation`. A partial context stays as sent. If no session exists, the push still inserts an empty session so the observation foreign key holds. `start_session` fills a context only when merging the stored object over the canonical empty `WorkContext` still equals that object. That includes the column default `{}` and an omitted-field object such as `{"files":[]}`. A real field or an extra key stays on the row. The first non-empty context stays, including when two starts race, and `ended_at` is left alone. `end_session` applies the session context only to in-memory copies of observations whose context is still empty, and only while building the summary. Stored observations and sync payloads are not rewritten. A partial context is not field-merged. Another person's row is not copied. An observation with no sync event is still rejected.

The runtime check is the evidence section below. The earlier source-only description of this finding is superseded for those tested cases. It is not superseded for old stored rows, secrets in repository, branch, worktree, or commit, or finding 7.

### 7. The hook drops the event when the server is down — device offline enqueue done for the verified opt-in; observe offline remains out of scope

Path: `scripts/cairn_hook.py`. Policy and the baseline say a failed provider must not discard a capture that still needs retry (`docs/policy.md:30`, `docs/contracts/agentmemory-baseline.md:143`). The outbox can retry. The device opt-in below is the enqueue. Observe and account deliveries are not queued.

**Credential transport, 2026-11-12:** fixed for an explicit client choice. The default is still `POST /cairn-memory/observe` with no bearer, for a file store with no accounts. `CAIRN_ACCOUNT_TOKEN` alone sends that account bearer only to observe. `CAIRN_DEVICE_TOKEN` alone sends that device bearer only to `POST /cairn-memory/sync/push`, for file device sync and for PostgreSQL, which has no observe route. Both set, or a value that is not one header token, is a configuration error before any request and before stdin is read. There is no generic token, no server detection, and no auth-error or redirect fallback. The device body is redacted with `cairn_outbox.normalize` before the request. Observe bodies are not client-redacted, so the file server still classifies the original content. The fingerprint stays `sessionId + source + content` from the input. The hook does not start or end a session or register a device. It writes a spool only for the opt-in device failure below. Tokens are environment values, not arguments, and are not stored in the payload. The client rule is [hook-v1](../contracts/hook-v1.md).

**Offline, device mode, 2026-11-12:** done for the opt-in checked below. `CAIRN_OUTBOX` unset still posts once and drops a failure. With that path and only `CAIRN_DEVICE_TOKEN`, a retryable failure (`URLError`, disconnect, `socket.timeout`, `TimeoutError`, incomplete read, HTTP 408, 429, or 5xx) appends the normalized body once and exits 0 only after the save, or when that event id is already in the same bound file. The origin is the version 2 canonical origin before the request. A `%` hostname, userinfo, query, fragment, or path is rejected first. An existing version 1 file, or a mismatched origin, route, or device, fails closed under `LOCK_NB` before the POST and is not purged. The append checks the binding again. The lock is not held during the socket call and is not waited on. A busy lock, a full queue, or a disk error exits 1, keeps the previous bytes, and does not say the capture was retained. A caller that ignores the nonzero exit can drop it. There is no second POST, no daemon, and no automatic replay. `CAIRN_DEVICE_TOKEN` is not read by replay. A 401 purges only after explicit replay has matched the version 2 identity. Observe and account deliveries are not queued even if the path is set. Their failure text does not claim the event was retained. An empty queued context inherits this person's session when replay is accepted, not at hook time. A partial context stays as sent. There is no rebind command. A new device token fails the fingerprint check and leaves the file unchanged. The old token still matches, so a 401 from that token still purges. Raw identity fields and unsupported context keys stay unredacted. Older spool files are not rewritten.

**Outbox serialization, 2026-11-12:** done for the current version 1 commands only. `enqueue`, `replay`, and `purge` take one exclusive sibling lock for the whole read, modify, and replace. `replay` holds that lock during the socket calls. `save` uses a unique same-directory temporary file and one replace. A failed replace keeps the previous queue. This is not hook offline enqueue.

Original trigger: `CAIRN_URL=http://127.0.0.1:9` and one JSON object on stdin. That run exited 1 and did not contact the review store. At that point the enqueue fix was still open. A connection error exited with `Cairn hook delivery failed` instead of an uncaught traceback. The payload was not printed. With `CAIRN_OUTBOX` unset, that drop remains. The device opt-in above saves a retryable failure.

Checks for this transport change, 2026-11-12, working tree only. No commit, dependency, schema, or live API.

| Check | Result |
| --- | --- |
| `python3 scripts/smoke_hook_auth.py StubHookTests` | Passed, 13 ok. Default observe has no bearer. Account observe keeps the bearer and the original content. Device sync uses the device bearer and redacts before the request. Both credentials, a spaced token, malformed JSON, and a bad context list do not post and do not print the token or the fixture secret. A 302 does not send a second request. Port 9 exits 1 with no traceback |
| `python3 scripts/smoke_hook_auth.py FileServerHookTests` | Passed, 1 ok, about 31s, temporary file store. Account observe stored `hook7a-account`. Device sync stored `hook7a-device` without the fixture secret. A device token on the account variable and an account token on the device variable returned 401 and left no observation |
| `./scripts/smoke_hook.sh` | Passed. Fresh file store, no account |
| PostgreSQL HTTP | Not run. The route difference is the source: `postgres_app` registers `sync/push` and does not register observe |

## Design alternatives, not defects

- Explicit `remember` mints a new id on every call (`crates/core/src/lib.rs:5172-5175`). A client retry creates a second durable memory. Add an idempotency key only if a caller retries today.
- The file rename is atomic as a directory entry and is not followed by `fsync` (`:4127-4135`). A power loss can still lose the last successful write. That is separate from finding 3.
- The hook fingerprint omits `role`. Two roles with the same content collapse to one event. Include role if both should be kept.
- Inheriting session context only when the observation context is completely empty is a footgun. Merge field by field.
- Purging the whole outbox on HTTP 401 matches the sync contract. Do not change it in this pass.
- Opening a file store tombstones expired observations (`crates/core/src/lib.rs:953-970`). A raw observation can expire before session end. That belongs to the lifecycle pass.
- `finish_session` sets the caller's `ended_at` before it knows the session has retained observations. An empty session returns `session has no retained observations` and leaves that in-memory `ended_at` set. The error path does not persist and does not roll the field back, so a later successful persist can write it. Other session rows are not changed. This predates choice B. It is a follow-up, not part of finding 4.

## Later tasks, one line each

- Retrieval: account search hides the unowned HTTP summary from finding 1, and a PostgreSQL summary can have an empty context even when the session row had a branch. Do not read those results as proof of what capture stored.
- Corrections and forgetting: not audited. Forget and expiry change what session end will include.
- Sharing: auto-share and promotion trust the sensitivity stored at capture, including a caller-supplied label.
- Promotion docs are stale and were not edited here. `docs/adr/0009-explicit-project-promotion.md` and `docs/contracts/project-layer4.md` say a promoted copy preserves files, symbols, commands, and tests, and they name only `promotion_reason` as redacted. The code redacts all seven human lists plus `next_steps` and `unresolved_work`.
- Old observation `content` copied into a new summary or promoted row does not pass through `redact_sensitive_content`. That republishes old content onto the new row. It is not a new-ingress hole and was not changed here.
- Sync, beyond this ingest edge: cursors, pull limits, and revocation were not re-audited. The person-scoped collision test passed.

## Remediation order

1. Require an account for file HTTP start, observe, end, and remember when accounts exist, and call the account-scoped methods. Done for the file HTTP handlers. Start uses `start_session_for_account`. Shared session context is choice B below.
2. Scope file event-id duplicates to the caller. Reject another person's id without returning the row. Library `capture_for_account` is done. File HTTP observe now calls that method whenever a bearer is required or presented.
3. Roll back a capture whose persist failed, so a retry writes instead of acknowledging a duplicate. Done for file `capture_observation`, `capture_for_account`, and `sync_push`. One rename publishes the observation together with its owner or sync event when that call includes them.
4. Let a person's own observed event gain a sync row, and keep the foreign-id rejection. Done for the file adapter when the persisted owner is that person. An unowned observation stays rejected.
5. Redact PEM blocks and case-insensitive `sk-` in the core and the outbox. Keep the stricter sensitivity label on the server. Core ingress is done. New outbox `content` is done. New outbox string items in the seven context lists are done. The outbox does not floor sensitivity. A new enqueue rejects a non-object context and a declared value that is not a list of strings, without putting that value in the error or the queue file. Old spools and old stored rows are not rewritten. Import metadata strings are redacted on the Rust import path only.
6. Keep PostgreSQL capture on device `sync/push`. Repair an empty session when a later account start arrives, and use session context on empty observation copies while building the summary. Done for the ordering tested below. Do not add `/observe`. Hook credential delivery is finding 7 and is partial. Offline enqueue remains open.
7. Make the hook enqueue on a retryable device failure when `CAIRN_OUTBOX` is set. Done for the checks below. Observe and account offline enqueue stay out of scope. Unset `CAIRN_OUTBOX` still drops.

## Evidence and checks

Read for this pass: `AGENTS.md`, `MEMORY.md`, `README.md`, `docs/README.md`, `RUST_IDIOMS.md`, `docs/policy.md`, `docs/contracts/hook-v1.md`, `docs/contracts/lifecycle-v1.md`, `docs/contracts/sync-layer3.md`, the capture sections of `crates/core/src/lib.rs`, `crates/server/src/lib.rs`, `crates/server/src/mcp.rs`, `crates/storage/src/lib.rs`, `migrations/0001_layer3_personal_memory.sql`, `scripts/cairn_hook.py`, `scripts/cairn_outbox.py`, and `crates/core/src/bin/cairn.rs`.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib` filtered to the six capture tests named below | Passed, 6 ok, 2.13s |
| `/tmp/cairn-capture-audit/harness` against temporary stores | `ALL_PASSED` (six cases) |
| `python3 /tmp/cairn-capture-audit/outbox_check.py` | Passed. `token=` redacted, `SK-` kept |
| Hook against `127.0.0.1:9` | Exit 1, uncaught `URLError`, no spool |
| PostgreSQL `crates/storage/tests/postgres.rs` | Not run. Needs `CAIRN_TEST_DATABASE_URL` |
| `cargo test --workspace`, smoke scripts, Clippy | Not run in the original audit |

Finding 2 library fix, checked after the `capture_for_account` change:

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib -- capture_for_account_duplicate_stays_with_the_owner` | Passed, 1 ok. Before the fix this failed: the member received `duplicate: true` and `owner private marker` |
| `cargo test -p cairn-core --lib -- duplicate_event_is_acknowledged_without_duplication` | Passed, 1 ok |
| `cargo test -p cairn-core --lib -- sync_event_id_collision_cannot_leak_another_persons_observation` | Passed, 1 ok |
| `cargo test -p cairn-core --lib -- two_accounts_keep_distinct_session_summaries_for_one_session_id` | Passed, 1 ok |
| `cargo test -p cairn-server --lib -- member_without_authorization_is_rejected_and_cannot_export_a_private_observation` | Passed, 1 ok. Bearer observe of a new event id still returns 200 |
| `cargo fmt -p cairn-core -- --check` and `cargo fmt --all -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |
| Workspace tests, smoke scripts, PostgreSQL | Not run for this fix |

The file HTTP observe route now uses `file_bearer`. Findings 3–7 were not changed.

Finding 1 file HTTP fix, checked after the handler change:

| Check | Result |
| --- | --- |
| `cargo test -p cairn-server --lib -- file_http_capture_uses_file_bearer_when_accounts_exist` | Passed, 1 ok, 5.74s |
| `cargo test -p cairn-server --lib` | Passed, 28 ok, 47.41s. Includes the new boundary test, the three project tests that now send the owner token on remember, the zero-account HTTP tests, and `member_without_authorization_is_rejected_and_cannot_export_a_private_observation` |
| `cargo test -p cairn-core --lib` for `capture_for_account_duplicate_stays_with_the_owner`, `duplicate_event_is_acknowledged_without_duplication`, `two_accounts_keep_distinct_session_summaries_for_one_session_id`, and `sync_event_id_collision_cannot_leak_another_persons_observation` | Each passed, 1 ok |
| `cargo fmt -p cairn-server -- --check` and `cargo fmt --all -- --check` | Passed |
| `cargo clippy -p cairn-server --all-targets -- -D warnings` | Passed |
| `./scripts/smoke_rest.sh` and `./scripts/smoke_hook.sh` | Passed. Fresh stores, no account before capture |
| `./scripts/smoke_web.sh` | Exit 1. The owner-bearer start, observe, and end curls ran. Next.js then exited because another `next dev` is already running for `apps/web` (pid 19480, port 3000). The console checks after that did not run |
| Isolated API on a free port, same capture calls as `scripts/smoke_web.sh` | Missing bearer on start returned 401 with only an `error` field. Owner bearer produced `memory-{person_id}-web-private` and `memory-{person_id}-web-project` |
| PostgreSQL tests and `scripts/smoke_postgres.sh` | Not run |
| Live `http://127.0.0.1:62200/cairn-memory/health` and `http://127.0.0.1:3000/api/cairn/health` | Both 200 before and after these checks. Neither process was restarted |

The fixed scope is the four file HTTP handlers, their server tests and the `scripts/smoke_web.sh` capture calls, plus ADR 0008, the Layer 3 auth contract, the hook contract, and one MCP sentence. CLI, MCP, hook authentication, sync, redaction, outbox, and PostgreSQL were not changed in that pass. Shared session context was still the first stored context until choice B below.

The six library tests were `redacts_sensitive_values_before_persistence`, `duplicate_event_is_acknowledged_without_duplication`, `capture_inherits_context_from_started_session`, `two_accounts_keep_distinct_session_summaries_for_one_session_id`, `sync_event_id_collision_cannot_leak_another_persons_observation`, and `invalid_observation_and_lifecycle_input_is_rejected_before_persistence`.

Finding 3 file capture commit, checked after the rollback change:

| Check | Result |
| --- | --- |
| The three new tests before the rollback | Failed. Each left the new observation in memory after the IO error |
| `cargo test -p cairn-core --lib` | Passed after the parent-directory correction, 31 ok, 14.28s. Includes the three commit tests, `duplicate_event_is_acknowledged_without_duplication`, `capture_for_account_duplicate_stays_with_the_owner`, `sync_event_id_collision_cannot_leak_another_persons_observation`, `trusted_device_sync_is_authenticated_idempotent_and_revocable`, and `two_accounts_keep_distinct_session_summaries_for_one_session_id` |
| `cargo test -p cairn-server --lib -- file_http_capture_uses_file_bearer_when_accounts_exist member_without_authorization_is_rejected_and_cannot_export_a_private_observation` | Passed, 2 ok, 5.28s |
| `cargo fmt -p cairn-core -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |
| Workspace tests, smoke scripts, PostgreSQL, server Clippy | Not run for this fix |
| Live API on port 62200 and Next.js on port 3000 | Not restarted. This fix is in the working tree only |

All three failure tests replace the temporary store directory with a file. `persist` then fails while creating that parent. It is not an injected `rename` error. The sync test sets the owner device `last_seen_at` to `baseline-seen`, persists that baseline, and asserts the failed push restores it. Owned and unscoped duplicates do not rewrite the file. A sync duplicate still rewrites it to store the device last-seen time. Findings 4–7 were not changed. AgentMemory remote save was skipped because the agentmemory MCP server required auth. No local memory daemon was started.

## Remediation checklist

- [x] Finding 1. File HTTP start, observe, end, and remember use `file_bearer` when accounts exist. Missing and invalid bearers are 401 and do not write. Observe, end, and remember call the account-scoped methods. Start calls `start_session_for_account`. Zero-account HTTP stays unscoped. The default hook has no bearer and receives 401 after bootstrap. `CAIRN_ACCOUNT_TOKEN` is an observe bearer, not a bypass of this gate.
- [x] Finding 2 library path. `capture_for_account` authenticates first, returns an owned duplicate, and rejects a foreign or unowned event id without the stored observation or a new owner record. Finding 2 did not change those duplicate rules. Finding 3 rolls back a failed persist.
- [x] Finding 3. A failed file rename drops the staged capture. Unscoped retry writes the observation once. Account capture commits the observation and owner together. Sync push commits the observation and sync event together. Reopen matches the file. Owned and unscoped duplicates do not write again. A foreign sync id is still rejected. Finding 4 is what adopts an owned observation that has no sync event. An unowned observation stays rejected.
- [x] Shared session context, choice B, approved 2026-11-12. A file account session is `(person_id, session_id)`. The public session id is unchanged. The same account's second start keeps the first context. Another account gets its own row and does not read or close the first account's context or `ended_at`. A missing `person_id` stays unscoped and is not adopted. `STORE_VERSION` stays 1. Account export filters by that person. Account import rejects another person's `person_id`. Unscoped import merges on the pair. CLI and MCP stay unscoped. Observations already copied from an older shared row are not rewritten. Findings 4–7 were not changed.
- [x] Finding 4. A file observation the device's person already owns, and that has no sync event, gains exactly one sync event. The stored observation is the payload. The first response is `duplicate: false` with a new cursor; a retry is `duplicate: true` with that same cursor. Foreign and unowned observations are rejected without the stored body and without a new owner or sync event. A failed adoption rename does not delete the observation or owner. Unscoped captures stay unowned and stay rejected. PostgreSQL is unchanged.
- [x] Finding 5 content redaction. Core capture, remember, correction, and sync normalize redact case-insensitive `sk-` tokens and recognized private-key PEM blocks, and the stored sensitivity is at least the label detected from the original text. New outbox `content` is redacted before the queue file is written. Older spools and stored rows are not rewritten.
- [x] Finding 5 metadata, new ingress. Rust new writes and new outbox queue files redact string items in the human-readable `WorkContext` lists before persistence. Import `next_steps`, `unresolved_work`, and `promotion_reason` are redacted on the Rust import path. Capture, remember, and correction raise sensitivity from the original text. Import keeps the bundle label. The outbox does not floor sensitivity; the server floor then sees the redacted text. Old rows and older spool files are not rewritten. A non-object context and a declared value that is not a list of strings are rejected before save. The error does not include the supplied value. Unknown keys and identity scalars are still not validated.
- [x] Finding 6 context ordering. An empty device push inherits this person's session when the incoming context is exactly empty. A later account start fills a still-empty session with the redacted request. The first non-empty context stays. Summary construction copies that session context onto empty observation copies only. Stored observations and sync payloads are not rewritten. No PostgreSQL `/observe` route: that is the approved policy. Device capture stays `POST /cairn-memory/sync/push`.
- [x] Finding 7 outbox serialization, version 1 only. `enqueue`, `replay`, and `purge` hold one exclusive sibling lock for the whole load, modify, and replace. `save` uses a unique same-directory temporary file and one replace. A failed replace keeps the previous queue. This is not hook offline enqueue.
- [x] Finding 7 version 2 device queue, primitives only. `enqueue_bound` creates a version 2 file only when that path is missing. Version 1 enqueue refuses a version 2 file. Replay checks origin, route, and device fingerprint before any socket. A mismatch leaves the file unchanged and does not purge. A matching 401 still purges. Token rotation has no rebind command. The hook appends only through the opt-in below.
- [x] Finding 7 offline enqueue, device mode only, verified 2026-11-12. `CAIRN_OUTBOX` plus `CAIRN_DEVICE_TOKEN` queues one retryable `sync/push` failure. Exit 0 happens only after the save or when the same event id is already in that bound file. Lock busy, a full queue, and a disk error exit 1 and do not retain the capture. Replay stays explicit, does not read `CAIRN_DEVICE_TOKEN`, and purges a 401 only after the version 2 identity matches. Observe and account offline enqueue are out of scope. An empty context inherits the session at accept time. A rotated token still mismatches. Raw identity fields and old spool bytes stay unsupported.
- [x] Account-scoped search limit, fixed 2026-11-13. See [CM-AUDIT-009](./2026-11-13-account-search-limit.md). Not a capture change.
- [ ] Account and anonymous offline enqueue, old-data republishing, and identity-field patterns stay open.

Finding 1's file HTTP authentication boundary is fixed. Finding 2's account duplicate scope is fixed. Finding 3's failed capture persist no longer acknowledges unsaved data. Shared session context uses choice B: independent file rows per `(person_id, session_id)`. Finding 4's file adoption is fixed: an owned observation with no sync event gains one sync event, and foreign or unowned ids stay rejected. Finding 5 content redaction is done for core ingress and new outbox `content`: case-insensitive `sk-` and recognized private-key PEM blocks are removed before persistence, and core keeps the stricter sensitivity label. Finding 5 Rust metadata redaction is done for new writes of the human-readable context lists and import handoff fields. New outbox string items in the seven context lists are redacted before the queue file is written. The outbox does not floor sensitivity. Old stored metadata is not migrated. A non-object context and a declared value that is not a list of strings are rejected before the queue file is written. The error does not include the supplied value. Unknown keys and identity scalars stay unvalidated. Finding 6 context ordering is fixed for the cases in the evidence section below. The absent PostgreSQL observe route is the approved policy. Finding 7 device-mode offline enqueue is done for the opt-in verified below. Explicit account observe and device sync delivery stay separated. Version 1 outbox commands serialize their replace, and version 2 replay still checks the binding before a socket. Observe and account offline enqueue remain out of scope. Unset `CAIRN_OUTBOX` still drops a failed hook delivery. CLI and MCP stay unscoped. The default hook stays unauthenticated. Unscoped capture still has no owner, so it is not adopted.

Choice B evidence, 2026-11-12, working tree only. No commit, no dependency change, no SQL migration.

| Check | Result |
| --- | --- |
| `cargo test --lib -p cairn-core -p cairn-server` | Passed. Core 32 ok, server 28 ok. Includes `account_sessions_isolate_context_for_one_public_id`, the adapted account end tests, and `file_http_capture_uses_file_bearer_when_accounts_exist` |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy -p cairn-core -p cairn-server --all-targets -- -D warnings` | Passed |
| `./scripts/smoke_rest.sh` and `./scripts/smoke_hook.sh` | Passed. Fresh stores, no account before capture |
| Isolated API on a free port, web capture calls plus two accounts on one session id | 13 PASS, 0 FAIL. Missing bearer was 401 with only `error`. Owner ends were `memory-{person_id}-web-private` and `memory-{person_id}-web-project`. The second account's start, empty observe, end scope, and export did not carry the first account's branch |
| `./scripts/smoke_web.sh` Next.js half | Not started. `apps/web/.next/dev/lock` is held by the existing `next-server` pid 19480 |
| PostgreSQL tests and Windows | Not run |
| Live API pid 59720 on port 62200 and Next pid 19480 on port 3000 | Still the processes started earlier. Health was 200 after this pass. Neither was restarted |

Observations that already copied a foreign context stay stored. This pass does not rewrite them. Findings 4–7 were not rechecked.

Finding 4 file adoption, checked after the sync change. Working tree only. No commit, no dependency change, no SQL migration, no live restart.

Before the fix, `owned_observation_adopts_one_sync_event` failed because a foreign owned event with no sync event returned `event already exists outside sync scope: member-event`. `failed_sync_adoption_keeps_the_observation_and_retries` then returned that same validation error on retry, so the owned event stayed outside sync.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib` | Passed, 34 ok, 2.67s. Includes the two new adoption tests, `sync_event_id_collision_cannot_leak_another_persons_observation`, `failed_sync_commit_does_not_keep_an_observation_without_sync_metadata`, `trusted_device_sync_is_authenticated_idempotent_and_revocable`, and `account_sessions_isolate_context_for_one_public_id` |
| `cargo test -p cairn-server --lib -- file_sync_adopts_an_owned_observation_once device_sync_registers_replays_once_rotates_and_revokes` | Passed, 2 ok, 0.94s |
| `cargo fmt -p cairn-core -p cairn-server -- --check` | Passed |
| `cargo clippy -p cairn-core -p cairn-server --all-targets -- -D warnings` | Passed |
| Workspace tests, smoke scripts, PostgreSQL | Not run |
| Live API pid 59720 on port 62200 and Next pid 19480 on port 3000 | Still those processes. Neither was restarted |

`duplicate: false` on the first attachment is the chosen reading of the existing flag. The contract already used `duplicate: true` for a push whose event id already has this person's sync event, returning that original cursor and creating nothing. The first attachment creates the sync event and a new cursor, so it is not that case. The retry is. PostgreSQL still rejects an observation that exists without a sync event. Findings 5–7 were not changed. `finish_session` still sets `ended_at` in memory before an empty session returns an error, and that field is not rolled back. That residual is recorded above and was not fixed.

The original audit did not open or write the review fixture `/tmp/cairn-ui-audit-desktop2/store.json`. Size stayed 30866 and mtime stayed Nov 12 12:47:31 2026. That pass used `/tmp/cairn-capture-audit/` only and made no source fix. The finding 2 library change above did not open that fixture, change dependencies, commit, or deploy.

AgentMemory remote save was skipped. The `agentmemory` MCP server required auth. No local memory daemon was started.

## Review runtime

Both processes were healthy at the end of this pass. Next was down at the start and was started from `apps/web` on this machine (`Andress-MacBook-Air`).

| Process | URL | Ready | Owner | Stop |
| --- | --- | --- | --- | --- |
| `cairn-server` pid 59720, parent 1 | `http://127.0.0.1:62200/cairn-memory/health` | 200, `file-backed-layer-3-prototype` | `andresholivin`, cwd this repo, store `/tmp/cairn-ui-audit-desktop2/store.json` | `kill 59720` |
| `npm` pid 19419, parent 1; `next dev` pid 19466; `next-server` pid 19480 | `http://127.0.0.1:3000/` | 200 | `CAIRN_API_URL=http://127.0.0.1:62200` | `kill 19419 19466 19480` |

`GET http://127.0.0.1:3000/api/cairn/health` returned the same file-backed health body, so the console is using the existing API. Log: `/tmp/cairn-capture-audit/next-dev.log`. Leave both running for the next review task.

Finding 5 core redaction, checked after the shared-helper change. Working tree only. No commit, no dependency change, no SQL migration, no live restart. The Python outbox was not edited.

Before the fix, `redacts_private_key_and_mixed_case_sk_and_keeps_sensitivity_floor` failed because case 0 still stored `SK-synthetic-example`. The fixtures are synthetic tokens and a synthetic PEM body. They are not credentials.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib redacts_private_key_and_mixed_case_sk_and_keeps_sensitivity_floor` before the fix | Failed. Observation content still contained `SK-synthetic-example` |
| Same test after the fix | Passed, 1 ok |
| `cargo test -p cairn-core --lib` | Passed, 35 ok, 4.02s. Includes the new regression, `redacts_sensitive_values_before_persistence`, `lessons_and_corrections_preserve_kind_and_supersession`, `import_rejects_unknown_version_and_redacts_payload`, and the capture, sync, and account tests |
| `cargo fmt -p cairn-core -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |
| `cargo test -p cairn-server --lib lifecycle_fields_and_diagnostics_work_over_http` | Passed, 1 ok. Server sources were not edited. HTTP observe still uses core normalize, and `password=` is still `secret` |
| `CAIRN_TEST_DATABASE_URL` and `crates/storage/tests/postgres.rs` | Unset, so PostgreSQL tests were not run. Source review: `sync_push` calls `normalize_observation`; remember and correct call `build_explicit_memory_record` |
| `scripts/cairn_outbox.py` | Not changed in the core pass. At that point it still redacted only the four markers |
| Workspace tests, smoke scripts, live API port 62200, Next.js port 3000 | Not run and not contacted |

The covered ingress is file capture, file sync, PostgreSQL sync normalize, explicit remember, and correction. A secret that appears only in a remember concept is redacted in the stored text and is not classified unless the content itself matches. Import redacts the new patterns and does not raise the bundle's sensitivity label. Stored files already on disk are not rewritten. At the end of the core pass the spool still had to learn the same case-insensitive `sk-` and private-key PEM redaction. The outbox pass below did that for new `content`. Findings 6 and 7 were not changed.

Finding 5 outbox content redaction, checked after `scripts/cairn_outbox.py` learned the core content patterns. Working tree only, on `Andress-MacBook-Air`. No Rust edit, no commit, no dependency or lock change, no live process, and no contact with ports 3000 or 62200. PostgreSQL was not run. AgentMemory MCP still required auth, so no remote checkpoint was saved.

Before the fix, `python3 scripts/smoke_outbox.py` failed 3 tests. The queue file bytes contained `SK-synthetic-example` and `c3ludGhldGljLWtleS1ib2R5`. The four markers in that same file were already `[REDACTED]`. These are synthetic tokens, not credentials.

| Check | Result |
| --- | --- |
| `python3 scripts/smoke_outbox.py` before the fix | Failed, 3 tests. Raw spool bytes still contained the synthetic `SK-` token and PEM body |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` after the fix | Passed, 3 tests, 1.532s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Rust tests, `scripts/smoke_layer3.sh`, live API, Next.js | Not run |

The check writes temp queue files and reads their raw bytes. A new spool omits the uppercase `SK-` token, the mixed-case forms, all five recognized PEM bodies (CRLF, mixed-case labels, a blank line between OpenSSL `DEK-Info` and base64, two blocks with prose between them, and a header with no `END`), and the four marker values. It keeps the event id, session id, repository, ordinary prose, a public certificate, a public key, UTF-8 neighbors, and the tail after an OpenSSH block with no `END`. Replay against a local stub returned `accepted: 2` in order and then an empty queue. A duplicate event id did not replace the first payload. The 257th event raised the 256 cap and was not written. A 401 purged both queued events. An older spool event was left unchanged.

At that pass a recognized truncated footer still kept key bytes glued to `-----END RSA PRIVATE KEY`. The footer correction below removes those bytes. `DSA PRIVATE KEY` is outside the five labels, so that body stays. A `BEGIN` that never closes with `-----` stays. At that point the spool still redacted only `content`. Findings 6 and 7 remain queued. Hook offline enqueue and hook authentication were not part of this pass.

Recognized glued truncated footer, checked after `truncated_footer_resume` in core and the outbox. Working tree only, on `Andress-MacBook-Air`. No dependency, schema, commit, live process, or old spool/store rewrite. The independent review file was not modified.

Before the change, the new spool content was `[REDACTED]`, a newline, then `c3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY`, then `next prose stays`. The Rust case failed the same way. `python3 scripts/smoke_outbox.py` failed that one assertion. These tokens are synthetic.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-core --lib redacts_private_key_and_mixed_case_sk_and_keeps_sensitivity_floor` before the footer change | Failed. Case 10 still contained the synthetic body |
| Same test after the change | Passed, 1 ok |
| `cargo test -p cairn-core --lib` | Passed, 35 ok, 1.86s |
| `cargo fmt -p cairn-core -- --check` | Passed |
| `cargo clippy -p cairn-core --all-targets -- -D warnings` | Passed |
| `cargo test -p cairn-server --lib lifecycle_fields_and_diagnostics_work_over_http` | Passed, 1 ok. Server sources were not edited |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` | Passed, 3 tests, 2.244s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| PostgreSQL, `scripts/smoke_layer3.sh`, live API port 62200, Next.js port 3000 | Not run and not contacted |

The spool check reads raw queue bytes. The glued line no longer contains the synthetic body or `-----END RSA PRIVATE KEY`, and `next prose stays` remains. Replay of that event posted the redacted content and left the queue empty. A same-line suffix, a mixed-case CRLF footer, a complete footer, and a header with no `END` plus following prose keep the prior safe behavior. An open `BEGIN` and `DSA PRIVATE KEY` still keep the synthetic body. Caller `public` on the glued capture is stored as `secret`, and the store file does not contain the body. At the end of the footer pass, raw context lists, older spools, finding 6, and finding 7 were still open. The metadata pass below closes new outbox context-list strings only. This is not a general secret scanner.

Finding 5 outbox metadata redaction, checked after `normalize` copies a `context` object and runs `redact_content` on string items in `files`, `symbols`, `commands`, `tests`, `services`, `handoffs`, and `runbooks` before `save`. Working tree only, on `Andress-MacBook-Air`. No Rust edit, no dependency or lock change, no commit, no live process, and no contact with ports 3000 or 62200. PostgreSQL was not run. AgentMemory MCP still required auth, so no remote checkpoint was saved.

Before the fix, `python3 -W error::ResourceWarning scripts/smoke_outbox.py OutboxRedactionCheck.test_context_lists_are_redacted_before_the_spool_write` failed in 0.000s. `normalize` returned the caller's context object, so the shared `files` list still held `SK-synthetic-example`. These tokens are synthetic.

| Check | Result |
| --- | --- |
| New context-list test before the fix | Failed. Normalized context was the input object |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` after the fix | Passed, 4 tests, 3.620s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Rust tests, PostgreSQL, `scripts/smoke_layer3.sh`, live API port 62200, Next.js port 3000 | Not run and not contacted |

The check reads raw queue bytes. Each of the seven lists drops `SK-synthetic-example` and `c3ludGhldGljLWtleS1ib2R5` and keeps the ordinary UTF-8 neighbor. `branch` stays `sk-looking-name`. Repository, worktree, commit, event id, session id, person id, `capturedAt`, and cursor stay. Caller `lifecycle.sensitivity` stays `public`. The caller's list is not mutated. Replay against a local stub posted the two events in order with redacted list strings and then left the queue empty. An older spool context list still contains `SK-prior-spool-example`. That pass still stored a non-object context and a non-string list item. The correction below rejects those shapes before a write. A missing `eventId` exits non-zero; stdout, stderr, and the spool do not contain the synthetic token or PEM body. The Python redactor does not floor sensitivity. Server normalize still does, on the text it receives, so it cannot classify the original secret. The documented label limit is unchanged. Findings 6 and 7 stay open. Hook authentication and offline enqueue were not part of this pass. Stale promotion wording in ADR 0009 and `docs/contracts/project-layer4.md`, and old content copied into a new summary or promoted row, are queued and were not edited.

Context-shape correction, after the review at `/tmp/cairn-outbox-metadata-review.md`. Working tree only, on `Andress-MacBook-Air`. No dependency, commit, live port, store write, or old-spool rewrite. AgentMemory MCP still required auth.

Before the change, `test_context_lists_are_redacted_before_the_spool_write` failed in 0.001s because a string `context` did not raise `ValueError`. `WorkContext` lists are `Vec<String>` with `serde(default)`, so a missing key is empty on the server and JSON `null` is not a vector.

| Check | Result |
| --- | --- |
| Context-list test before this rejection | Failed. `ValueError` was not raised |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` after the change | Passed, 4 tests, 3.369s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Rust tests, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |

A string or list `context`, a scalar `files` value, a dict item, a nested list item, and a null `commands` or `symbols` list each raise before `save`. The exception text is `outbox context must be a JSON object` or `outbox context <field> must be a list of strings`. CLI stdout and stderr for those enqueues do not contain `SK-synthetic-example` or the synthetic PEM body. An existing queue file with `evt-kept` and `src/kept.rs` is byte-for-byte unchanged, so the kept event is not dropped. A new path `rejected-new.json` is not created. Omitted and null context, UTF-8 list strings, `sk-looking-name`, ids, the 256 cap, duplicate event ids, replay order, and 401 purge still pass. Unknown keys and identity scalars are still not validated. Older spool files are not migrated. Findings 6 and 7, hook offline enqueue, and old content republished into a new summary or promoted row stay separate.

Finding 6 context ordering, checked after the `PostgresStore` change. Working tree only, on `Andress-MacBook-Air`. No dependency, lock, schema, route, commit, or live port 3000 or 62200 change. The file adapter was not edited. AgentMemory MCP still required auth, so no remote checkpoint was saved.

Before the fix, the same ignored test failed in 0.88s. `sync_push` after `start_session` returned an observation whose repository was missing. That run used `PostgresStore` on disposable database `cairn_ctx_order_20261112_0c590134`. The database was dropped and created again before the passing run. No image was pulled and the container port was not changed.

| Check | Result |
| --- | --- |
| `cargo test -p cairn-storage --test postgres -- --ignored postgres_context_order_repairs_empty_session_without_rewriting_observations` before the fix | Failed. Inherited repository was missing |
| Same test after the fix | Passed, 1 ok, 1.02s |
| `cargo fmt -p cairn-storage -- --check` | Passed |
| `cargo test -p cairn-storage --lib` | Passed, 1 ok |
| `cargo clippy -p cairn-storage --all-targets -- -D warnings` | Passed |
| Disposable database `cairn_ctx_order_20261112_0c590134` | Created for the repro, recreated for the pass, then dropped. Remaining count for that name was 0 |

The passing test calls `start_session`, `sync_push`, `end_session`, `import_bundle`, and `export_bundle`. Start then empty push stores the session repository, branch, and worktree on the observation and on the worktree-scoped summary. Empty push then start leaves the stored observation and the duplicate sync response empty, fills the session, and puts that context on the summary. A second non-empty start does not replace the first. Two concurrent starts on one empty session return the same context, and a later start does not replace it. The other person's push of the same public session id does not receive the first person's branch. A context that sets only `files` stays partial. A duplicate returns the same cursor and the stored context. An imported observation with no sync event is rejected with `event already exists outside sync scope` and the error does not include the observation body. A session file containing `password=` does not retain the fixture secret, and the export does not contain it. `ended_at` stays set after a later start.

The source-only caveat for this ordering is superseded for those cases. Old rows already stored are not rewritten. A secret in repository, branch, worktree, or commit is still stored. A partial context is still not field-merged. The default hook still posts `/observe`, and a down server still drops the capture. Finding 7 credential transport is partial. `cargo fmt -p cairn-storage -- --check` passed. `cargo test -p cairn-storage --lib` passed, 1 ok. `cargo clippy -p cairn-storage --all-targets -- -D warnings` passed. The long PostgreSQL flow, server tests, and smoke scripts were not rerun. Main still needs an independent review before acceptance.

Empty-context predicate correction, after the review at `/tmp/cairn-ctx-order-review.md`. Working tree only, on `Andress-MacBook-Air`. No dependency, lock, schema, route, commit, or live port change. The same ignored test was extended. It was not a new route.

Before the predicate change, that test failed in 1.11s. `start_session` on a column-default `{}` row returned an empty `WorkContext`. The database was `cairn_ctx_pred_20261112_5f390110`. It was dropped and created again before the passing run, then dropped. Remaining count for that name was 0. A separate probe database `cairn_ctx_pred_probe_5f390110` checked `canonical || stored = canonical` and was dropped. Its remaining count was 0.

| Check | Result |
| --- | --- |
| Same ignored test before `($4::jsonb \|\| context) = $4::jsonb` | Failed, 1.11s. Column-default start returned an empty context |
| Same test after the predicate change | Passed, 1 ok, 1.12s |
| `cargo fmt -p cairn-storage -- --check` | Passed |
| `cargo test -p cairn-storage --lib` | Passed, 1 ok |
| `cargo clippy -p cairn-storage --all-targets -- -D warnings` | Passed |
| Disposable database `cairn_ctx_pred_20261112_5f390110` | Recreated for the pass, then dropped. Remaining count 0 |

The passing run still covers the earlier ordering. It also reads raw jsonb text. The column default is `{}` before start and the canonical object after, and `start_session` returns the supplied context. `{"files": []}` does the same. `{"branch": "kept-branch"}` and `{"extra": "keep-me", "files": []}` are unchanged. Two concurrent starts on one `{}` row return the same context, and a later start does not replace it. The other account's row is a different text. The observation jsonb text and the duplicate sync response stay the original empty observation. No `/observe` route was added. Finding 7 offline enqueue stays open. A non-object context is not treated as empty. Serde still ignores an unknown key on read, so the returned `WorkContext` does not show it; the stored text keeps it.

Outbox replay redirect block, checked after `scripts/cairn_outbox.py` stopped following redirects on `POST /cairn-memory/sync/push`. Working tree only, on `Andress-MacBook-Air`. No dependency, lock, schema, commit, or live port 3000 or 62200 change. The hook file was not edited. Queue files stay version 1. AgentMemory MCP still required auth, so no remote checkpoint was saved.

Current replay, including a version 1 queue, fails closed on 301, 302, 303, 307, and 308. It does not request the redirect target or forward the bearer. The old version 1 `urlopen` behavior followed 301, 302, and 303 and could send `Authorization` to another origin. That compatibility behavior is closed on purpose. A redirect leaves the event queued, with the payload unchanged and one added attempt. Only a 401 purges. The stored error and command output omit the Location URL and the response body. A later queue version must fail closed the same way. This pass does not add queue version 2, file locking, or offline hook enqueue.

Before the fix, `OutboxRedirectCheck` failed three subtests in 4.867s. For 301, 302, and 303 the second origin received one request. A separate local `urlopen` probe, using a synthetic bearer and two loopback stubs, showed that request was a GET whose `Authorization` matched the bearer sent to the first origin, and whose body was not the queued event. 307 and 308 raised `HTTPError` and did not contact the second origin. No token value is recorded here.

| Check | Result |
| --- | --- |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py OutboxRedirectCheck` before the fix | Failed, 3 subtests (301, 302, 303), 4.867s |
| Same statuses through `urllib.request.urlopen` on two loopback stubs | 301, 302, and 303 forwarded the bearer. 307 and 308 did not |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` after the fix | Passed, 5 tests, 8.377s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Hook `StubHookTests`, Rust, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |

After the fix, each of 301, 302, 303, 307, and 308 produced one POST to the first origin's `/cairn-memory/sync/push` and zero requests to the other origin. The queue version stayed 1, the payload was unchanged, attempts became 1, and the event was not purged. Replay stdout, stderr, and the queue file did not contain the synthetic bearer, the Location marker, the redirect body marker, or the target URL. The existing acceptance-order and 401-purge checks still passed. Main reviews this before queue locking or version 2.

Redirect error correction, after review `/tmp/cairn-outbox-redirect-review-task_27bd374975e3.md`. The cross-origin forward stayed closed. Parent `http_error_302` still ran first for a Location scheme other than `http`, `https`, `ftp`, or empty, and stored that raw URL in `lastError`. `redirect_request` also read the body, so a short body aborted before `save` and a held-open body could block. Working tree only, on `Andress-MacBook-Air`. No dependency, lock, schema, commit, hook edit, or live port 3000 or 62200. Queue version 2, locking, and offline enqueue were not added.

`http_error_302` now closes the response without reading it and raises `HTTP Error <code>: redirect blocked` with no Location, header, or body. `301`, `303`, `307`, and `308` alias that method. It does not call the parent. A missing Location uses the same fixed pending error. `200` acceptance and `401` purge are unchanged.

Before this correction, `OutboxRedirectCheck.test_unsupported_scheme_omits_the_location_from_the_stored_error` failed all five statuses in a 6.065s run, and `test_redirect_close_does_not_read_or_stall` errored because the short-body replay printed no JSON summary. The failure text did not include the synthetic bearer. No token value is recorded here.

| Check | Result |
| --- | --- |
| New redirect tests before this correction | Failed, 5 subtests plus 1 error, 6.065s |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` after the correction | Passed, 7 tests, 15.976s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Hook tests, Rust, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |

After the correction, `abc://` Locations for 301, 302, 303, 307, and 308 made one origin POST and zero target requests. The queue stayed version 1, the payload was unchanged, attempts became 1, and stdout, stderr, and the queue omitted the bearer, the Location, and the body. A short body and a held-open body closed without `IncompleteRead` or a timeout. A 302 with no Location stayed pending with the fixed error. Acceptance order, the 256 cap, context validation, and 401 purge still passed. Main reviews this before locking.

Version 1 outbox lock and atomic replace, checked after `scripts/cairn_outbox.py` stopped sharing one `.tmp` name and took `fcntl.flock` on the sibling `<queue>.lock` for `enqueue`, `replay`, and `purge`. Working tree only, on `Andress-MacBook-Air`. No dependency, schema, commit, hook edit, or live port 3000 or 62200. Queue files stay version 1. No version 2 file and no hook offline enqueue. AgentMemory MCP still required auth, so no remote checkpoint was saved.

Before the change, `OutboxLockCheck` failed 6 tests and errored 2 in 1.096s. Eight processes started together lost ids or raised `FileNotFoundError` on the shared `queue.json.tmp`. A purge beside an enqueue did the same. An enqueue finished while a replay stub still held the HTTP response. A forced `os.replace` error left a temporary file that contained `synthetic-save-body`. These tokens are synthetic.

| Check | Result |
| --- | --- |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py OutboxLockCheck` before the change | Failed, 6 failures and 2 errors, 1.096s |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` after the change | Passed, 15 tests, 21.822s, real 22.12s, exit 0 |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Hook tests, Rust, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |
| Linux and Windows | Not executed. A missing `fcntl.flock` refuses the write |

After the change, eight processes enqueueing distinct ids kept all eight. Eight processes enqueueing one id stored that id once, and only one reported it queued. An enqueue started while replay was blocked in a loopback 500 waited, then the queue held the original event with one attempt and the new event. Purge of the old id beside enqueue of a new id left only the new id. A failed replace left the previous bytes and mode, removed the temp file, and a later enqueue still acquired the lock. A new queue file is mode `0600`. An existing `0640` file stayed `0640`. The lock file stayed the same inode and was not treated as a queued event. Same-thread reentry locked once. `status` is not locked. The earlier content, metadata, and redirect tests passed in that same 15-test run. The lock wait has no timeout. One replay can hold it for up to 256 attempts of 10 seconds each. Main reviews this before a version 2 queue. Offline hook enqueue stays open.

Version 2 device binding and the inode reentry fix, checked after `enqueue_bound` and version-aware `replay` in `scripts/cairn_outbox.py`. Working tree only, on `Andress-MacBook-Air`. No dependency, schema, commit, hook edit, or live port 3000 or 62200. No rebind command. The hook still does not enqueue. AgentMemory MCP still required auth, so no remote checkpoint was saved.

Same-thread reentry is now `(st_dev, st_ino)` of the opened lock file. A nested call through `/tmp` and `/private/tmp`, or another symlink to the same directory, closes the extra descriptor and does not unlock the outer owner. Another process stays locked until the outer block exits. `load` and `save` stay unlocked. The queue path passed to `os.replace` is not realpath'd.

`enqueue_bound` creates version 2 only when the file is missing. The file stores `route: sync/push`, a normalized origin, and SHA-256 of `cairn-outbox-v2`, a NUL, and the raw device token. It does not store the token. Content and context-list redaction still run before `save`. A bad origin, a token equal to `CAIRN_ACCOUNT_TOKEN`, or a failed replace leaves no queue file and no payload temp. Version 1 `enqueue` refuses a version 2 file. `enqueue_bound` refuses a version 1 file. Neither rewrites the other.

Version 2 `replay` checks the account-token rule, metadata, route, origin, and fingerprint before any socket. A wrong origin, token, route, version, or invalid metadata leaves the bytes unchanged and does not purge. A matching 401 purges the events and keeps the binding. Accepted events, including `accepted: true`, are removed in order. Redirects still use the existing no-redirect opener. `status` and `purge` do not print the fingerprint. There is no rebind command. A new token does not match. The old token still matches, so its 401 still purges. One file is one token and one origin. Recovery is a new file.

Version 1 replay still does not read `CAIRN_ACCOUNT_TOKEN`, including when that value equals `--token`. It still has no fingerprint field. The hook file was not edited.

| Check | Result |
| --- | --- |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` | Passed, 20 tests, 29.986s, exit 0. The previous 15 tests are in that run |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Hook tests, Rust, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |
| Linux and Windows | Not executed. Platform was macOS 27.2 arm64, Python 3.9.6 |

Offline hook enqueue stays open. The next task is the hook's transient device enqueue. It must not take this lock on the hook's foreground stack while replay can be inside a socket call.

Percent-host rejection, checked after `normalize_origin` refuses a hostname that contains `%`. This is separate from the `/tmp` versus `/private/tmp` lock-inode fix. Working tree only, on `Andress-MacBook-Air`. No dependency, commit, hook edit, or live port. The check does not decode the host. `http://127.0.0.1%2eattacker.example:3111`, the `%2E` form, `http://evil.example%3a8443`, `http://127.0.0.1%00.example`, and `http://[fe80::1%25lo0]:3111` plus `http://[fe80::1%lo0]` each raise `outbox origin must be an http(s) server base with no userinfo, query, fragment, or path`. That text does not contain the URL or the token. `enqueue_bound` creates no file. Replay and `enqueue_bound` leave an existing version 2 file byte-for-byte unchanged. The patched opener was not called. `localhost`, default ports, trailing slashes, and `[::1]` still normalize. An IPv6 zone id is unsupported on purpose.

Left unchanged, and not fixed here: `version: true` is still version 1 because `True == 1`. Version 1 replay still does not use this normalizer. Unicode, fullwidth dots, IDNA, and a literal backslash host are still the previous behavior. There is still no nonblocking lock acquire.

| Check | Result |
| --- | --- |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` | Passed, 21 tests, 29.247s, exit 0. Includes the previous binding, 401 purge, version 1 coexistence, and inode alias tests |
| `python3 -m py_compile scripts/cairn_outbox.py scripts/smoke_outbox.py` | Passed, exit 0 |
| Hook tests, Rust, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |
| Linux and Windows | Not executed. Platform was macOS 27.2 arm64, Python 3.9.6 |

Device-mode offline enqueue, checked after `scripts/cairn_hook.py` appends a retryable `CAIRN_DEVICE_TOKEN` failure when `CAIRN_OUTBOX` is set. Working tree only, on `Andress-MacBook-Air`. No dependency, schema, commit, Rust edit, or live port 3000 or 62200. `enqueue_bound` gained `nonblocking` and `report`. `preflight_bound` checks an existing file under `LOCK_NB` and does not create a missing queue. CLI enqueue, replay, and purge still block. The hook does not hold the lock during the POST and does not wait. A busy lock exits 1 with `the capture was not retained`. A full queue and a directory that rejects the temporary file keep the previous bytes and leave no payload temp. Connection refusal, HTTP 408, 429, 500, and 502, an incomplete read, and a stall after the stub stored the id each produced one POST and one version 2 event. The queued content and the seven context lists were redacted. The token and fingerprint were absent from stdout and stderr. Explicit replay with the bound token, while `CAIRN_DEVICE_TOKEN` in the environment was a different value, accepted the duplicate once and did not replace the stored body. A second replay posted nothing. Permanent HTTP statuses, including a 302 to another port, did not queue and did not touch the second port. Anonymous and account failures with the path set did not call `sync/push` and did not create a queue. Both tokens made no request. A percent-encoded host, a path, a query, and userinfo were rejected before a POST and before a lock file. Version 1, a wrong origin, a wrong token, another route, and version 3 stayed byte-for-byte with zero posts. Two concurrent device hooks kept every id that exited 0. An id that did not fit the lock was a nonzero exit and was not reported as queued. Observe offline enqueue was not built. Empty-context inheritance at accept time was not replayed through PostgreSQL. `version: true` is still version 1. Identity scalars and old spool bytes were not rewritten.

| Check | Result |
| --- | --- |
| `python3 -W error::ResourceWarning scripts/smoke_hook_offline.py` | Passed, 11 tests, 24.544s, exit 0 |
| `python3 -W error::ResourceWarning scripts/smoke_outbox.py` | Passed, 21 tests, 28.128s, exit 0 |
| `python3 -W error::ResourceWarning scripts/smoke_hook_auth.py StubHookTests` | Passed, 13 tests, 2.197s, exit 0 |
| `python3 -W error::ResourceWarning scripts/smoke_hook_auth.py FileServerHookTests` | Passed, 1 test, 40.068s, exit 0, temporary file store |
| `./scripts/smoke_hook.sh` | Passed. Fresh file store, no account |
| `python3 -m py_compile scripts/cairn_hook.py scripts/cairn_outbox.py scripts/smoke_hook_offline.py` | Passed, exit 0 |
| Rust, PostgreSQL, live API port 62200, Next.js port 3000 | Not run and not contacted |
| Linux and Windows | Not executed. Platform was macOS 27.2 arm64, Python 3.9.6 |
