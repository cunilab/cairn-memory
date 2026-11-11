---
id: CM-ADR-0011
title: "Explicit project memory share"
status: accepted
---

# ADR 0011 — Explicit project memory share

## Context

Layer 4 promotion creates a project-scoped decision that still belongs to one
person. A second account must be able to read a chosen project memory without
receiving the owner's private observations, personal memories, or unshared
project memories. Sharing has to be an explicit owner action that can be
revoked, and the grant has to stay inspectable.

## Decision

1. Share only a project memory the owner has already promoted. Sharing a
   personal observation or a non-project memory fails.
2. The owner invites another account by email. Any active account session of
   that member can read the shared memory. The member cannot read the owner's
   private observations, personal memories, or unshared project memories.
3. Only the owner can share, revoke, correct, or forget that memory. The
   member can read.
4. Revoke makes a later read by the member omit that memory. The memory stays
   stored for the owner. This slice does not implement offline-device purge,
   encryption, or delete. Automatic sharing is limited to the explicit policy
   in the follow-on below.

A live successor that supersedes a shared memory is omitted from the member
read. The grant stays on the original id, and the owner must share the
successor explicitly. Invite, share, and revoke write owner audit events
`project_member_invited`, `project_memory_shared`, and
`project_memory_share_revoked`. Those records may contain the project id,
member person id, and memory id. They do not store memory content, tokens, or
passwords.

The file-backed store keeps `STORE_VERSION` 1 and loads the new vectors with
serde defaults. PostgreSQL stores membership and shares in
`migrations/0008_layer5_project_shares.sql`. A share does not copy the memory
into the member's personal memory list and does not add it to device sync pull.

## Follow-on: explicit share policy

The collaborative specification already required automatic sharing to fail
closed unless an explicit policy matches. This follow-on does not add roles.

1. The default project policy is `manual`. Manual promotion creates no share.
2. The owner may set `auto_nonsensitive_project_memory`. Promoting a
   project-scoped memory that is not secret and not sensitive then shares it
   with each current member and writes `project_memory_shared`.
3. Share candidates are promoted project memories that are not secrets, not
   personal observations, and not actively shared with that member. Each
   candidate explains why it is a candidate and why a manual policy does not
   auto-share it. Dismissing a candidate does not share it. Accepting it uses
   the existing share method.
4. Turning the policy off does not revoke existing shares.
5. Contradictory live project memories stay stored. The owner and the member
   can read a conflict list of both ids. A member only sees conflicts among
   memories shared with them.
6. A shared read includes `share_reason` and `source_memory_id` when those
   values exist. Session summaries keep source actors and source event ids.
   Branch merge keeps `promoted_from` and `source_event_ids`.

## Amendment: project team space and retained delete

The project cairn remains the shared space. A team space belongs to one
project. The only roles are owner and member. The owner creates the team,
names it, and adds or removes an existing project member. A member can list
the team and its members. A person who is not a member cannot. Removing a
member removes that project membership and revokes active shares for that
project. A later shared read by the former member is empty. The memory stays
stored for the owner.

Delete is owner-only. It hides the memory from normal recall with the same
tombstone as forget, keeps the stored record and the export copy, and writes
`memory_delete_retained`. It does not purge other devices. A member delete
fails with the existing not-owner error.

Creating a team, adding a member, and removing a member write
`project_team_created`, `project_team_member_added`, and
`project_team_member_removed`. Account promote, share, revoke, and forget
write `memory_promote`, `project_memory_shared`,
`project_memory_share_revoked`, and `memory_forget`. Audit metadata may
contain the project id, member person id, and memory id. It does not contain
memory content or tokens.

Offline purge, encryption, federation, and CRDTs stay unimplemented.

## Consequences

- The first Layer 5 slice is one owner, one invited member, and one explicit
  memory grant. The team space and retained delete are the amendment above.
  Offline purge, encryption, federation, and CRDTs remain later work. Roles
  beyond owner and member are not part of this model. Automatic sharing is
  only the explicit nonsensitive project-memory policy above.
- Repeating an invite, or an active share of the same memory to the same
  member, is idempotent. Unknown emails and self-invites fail closed.
- Member correction and forget use the existing validation path. Where the
  server already maps a message containing "does not own" to 401, that status
  stays in place.

## Evidence

- `crates/core/src/lib.rs`
- `crates/storage/src/lib.rs`
- `crates/server/src/lib.rs`
- `migrations/0008_layer5_project_shares.sql`
- `migrations/0009_layer5_share_policy.sql`
- `migrations/0010_layer5_team_space.sql`
- `docs/contracts/share-layer5.md`
