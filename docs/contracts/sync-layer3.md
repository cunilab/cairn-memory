---
id: CM-CONTRACT-007
title: "Layer 3 Trusted Device Sync"
status: draft
---

# Layer 3 Trusted Device Sync

This contract defines the first local Layer 3 slice: person-scoped trusted
devices, bounded authenticated replay, durable cursors, token rotation, and
revocation.

## Device identity

`POST /cairn-memory/devices/register` creates or resumes a device registration:

```json
{
  "deviceId": "device-macbook",
  "personId": "person-1",
  "agentId": "agent-a",
  "name": "MacBook"
}
```

The response contains a device record and an opaque device token after the
account bearer token authorizes the requested `personId`. The device token is
supplied as:

```text
Authorization: Bearer <device-token>
```

The file-backed adapter stores the token in local state for compatibility. The
PostgreSQL adapter stores only a token hash. OIDC, key management, encrypted
caches, and offline purge guarantees remain later deployment work.

## Device operations

| Operation | Endpoint | Authorization |
| --- | --- | --- |
| Register/resume | `POST /cairn-memory/devices/register` | Active account token matching `personId` |
| List same-person devices | `GET /cairn-memory/devices` | Active device token |
| Rotate a token | `POST /cairn-memory/devices/rotate` | Active same-person token |
| Revoke a device | `POST /cairn-memory/devices/revoke` | Active same-person token |

Device records expose `status`, `registered_at`, `last_seen_at`, and a durable
sync cursor. Revoked devices receive `401` for authenticated sync operations.

## Sync operations

### Push

`POST /cairn-memory/sync/push` accepts the normal observation payload with an
active device token. Secret redaction and normal Layer 1 validation happen
before the event is added to the personal sync stream.

The response includes:

```json
{
  "accepted": true,
  "duplicate": false,
  "cursor": 1,
  "observation": {}
}
```

Pushing the same `eventId` again returns `duplicate: true` and the original
cursor without creating another observation or sync event.

### Pull

`POST /cairn-memory/sync/pull` accepts `{ "cursor": 0 }` and returns all
authorized events for the same person after that cursor. The returned cursor is
the value a client persists and sends on the next pull. Repeating a pull with
the returned cursor is empty.

### Diagnostics

`GET /cairn-memory/sync/diagnose` accepts an active device token and reports
the device cursor, person stream cursor, lag, pending/rejected counters, and a
recovery hint. A non-zero lag means the client should pull from its last
persisted cursor. Rejected events remain visible in the local outbox status
until repaired or purged.

## Local outbox

`scripts/cairn_outbox.py` provides a bounded local spool:

```bash
printf '%s\n' '{"eventId":"evt-1", ...}' \
  | python3 scripts/cairn_outbox.py enqueue /tmp/cairn-outbox.json
python3 scripts/cairn_outbox.py replay /tmp/cairn-outbox.json \
  --url http://127.0.0.1:3111 --token '<device-token>'
python3 scripts/cairn_outbox.py status /tmp/cairn-outbox.json
```

The spool is bounded to 256 events, redacts common secret markers before local
persistence, removes only server-acknowledged events, and retains transiently
failed items with attempt/error metadata for retry or repair. A `401` during
replay means the device credential is revoked or invalid; the replay command
purges the queued events for that credential. Operators can also run
`cairn_outbox.py purge` explicitly.

## Safety and boundary

- Sync is person-scoped; a device cannot pull another person's events.
- Revocation stops future authenticated pull and push operations.
- Device records and tokens are not part of the Layer 1 memory export bundle.
- Clients may cache only redacted, bounded outbox payloads and the last
  acknowledged cursor. They must not persist account tokens in the outbox or
  cache server memory responses without an explicit encrypted-cache policy.
- A server forget or retained delete keeps the record in storage. Export still
  contains the forgotten record and its forgotten id. Normal recall hides it.
- Sync pull does not return an observation or memory the person has forgotten.
  A device that already pulled the item receives a deletion instruction on its
  next pull. A device that never pulls again is not purged.
- A revoked device must discard its token and queued outbox material, with
  repair requiring a newly authorized device token.
- The file-backed adapter proves local compatibility behavior. The PostgreSQL
  adapter provides the server-authoritative account, device, observation, and
  sync-event path.

## Evidence

- `crates/core/src/lib.rs` stores devices, cursors, sync events, rotation, and
  revocation.
- `crates/server/src/lib.rs` exposes the authenticated REST boundary.
- `crates/storage/src/lib.rs` implements the PostgreSQL Layer 3 repository.
- `migrations/0001_layer3_personal_memory.sql` defines the canonical tables.
- `crates/core/src/lib.rs` and `crates/storage/src/lib.rs` expose sync diagnostics.
- `scripts/cairn_outbox.py` implements the bounded local replay spool.
- `scripts/smoke_layer3.sh` proves registration, replay, deduplication, pull,
  cursor resume, rotation, last-seen state, and revocation with real processes.
