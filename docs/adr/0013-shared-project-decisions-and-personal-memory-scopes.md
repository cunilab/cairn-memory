---
id: CM-ADR-0013
title: "Shared project decisions and personal memory scopes"
status: proposed
---

# Shared project decisions and personal memory scopes

Date: 2026-11-16.

## Context

A member needs the same published project knowledge in the console and in agent
recall. Per-person grants, automatic share policies, optional team spaces and
repository-name matching produce different answers for members and can confuse
private captures with published decisions. Pre-release development stores do not
need data migration or compatibility with the previous project model.

This decision supersedes the sharing, private-project registration and
personal-origin suppression parts of ADR 0011 and ADR 0009, and the team-space
and visibility-escalation parts of ADR 0002. ADR 0010's linked-history rule remains.

## Decision

1. Every live published project decision is readable and usable by every member,
   including the owner. Publication is an explicit transition that sets
   `published_by`; a `project_id`, repository tag, memory kind or scope alone is
   never proof of publication. Imports clear publication attribution and project
   publication links. Private captures and another person's personal memories
   never enter project lists or shared recall.
2. Personal memory has two parts. A memory with `scope: global` has no repository
   context and follows its person everywhere: use explicit remember/lesson save
   without a repository for preferences, habits, lessons and working style. Global memories cannot be promoted. A
   remember/lesson with repository context stays private to that repository.
   Capture summaries retain the existing branch/worktree continuity scopes;
   those scopes govern relevance, never cross-person access. All parts are private.
3. Only `cairn init` explicitly creates a project and writes
   `.cairn/project.json`. Captures without that file create no project. A valid
   existing file makes init join that project and report "already initialized,
   you are a member". Capture also joins by project ID, idempotently. Failed
   attachment is reported in the capture response and retried on the next capture,
   including a duplicate replay. Broken or unreadable local files warn in the
   CLI/hook, and accepted captures remain personal. Unknown IDs warn; they never
   create a substitute project. Ownerless legacy projects have no supported access.
4. Repository identity is `host/org/repo`, with SSH and HTTPS forms normalized,
   the host lowercased and `.git` removed. Without a network remote it is
   `local:<directory>`, explicitly marked local. The UI uses the short project
   name. A renamed remote does not break publication of personal sources tagged
   with the committed project ID; those sources also remain in the Memories tab.
   The server URL stays in client configuration, outside the committed file;
   a file for a different server reports an unknown project instead of creating one.
5. Any member may publish their own live repository memory. Secret-marked sources
   are refused. The source stays private. Preview shows the full copied content,
   lifecycle and metadata before publication; CLI/MCP expose `dryRun` and the CLI
   has `promote-preview`. The copy clears personal event/session provenance and
   branch/worktree context, retains verification evidence, and records its author.
   Generated publication identifiers and timestamps are allocated on each request.
   A preview fingerprint binds copied fields to publication; the console refuses
   changed summaries and asks for another preview rather than publishing unseen data.
6. The author and current project owner may verify, correct and delete a decision.
   Other members read only. Corrections are themselves published decisions and
   retain the original author, with `changed_by` and `changed_at` identifying the
   actor. Storage ownership stays with the original author, so a former owner
   cannot use a personal import to hide the decision after transfer.
   Deletion is retained, with actor attribution. Audit history stays private
   to its account. Ordinary verification/correction/deletion works for both web
   sessions and device tokens; ownership transfer is a web-session operation.
7. Normal search, context, recall and why-changed rank eligible personal memories
   and project decisions together. Each result has `source: personal | project`.
   Project selection requires the project ID in query context, never a matching
   repository short name. Resume and unresolved-work stay personal. A repository-only query includes private branch/worktree captures; explicit
   branch/worktree filters continue to narrow personal continuity results.
8. A private source correction, expiry, forget or deletion cannot change or hide
   a publication. An explicit later promotion of a corrected source may supersede
   its earlier publication. A live successor hides its predecessor; forgetting or
   expiring the successor can reveal the predecessor again, as in ADR 0010.
   Lists, recall and maintenance use the same live-publication eligibility rule.
9. The owner and an administrator can transfer ownership to an existing member.
   The old owner stays a member, author attribution is preserved and an audit
   event records the actor and both owners. Owner-only project entities and
   verification history pass to the new owner; personal audit history stays private.
   PostgreSQL serializes transfer with
   decision mutations. Account deletion and project deletion workflows are deferred.
10. Branch/worktree records and merge/close status belong to the person who records
    them. A member can register their own branch, and identical branch names are
    independent. "Record merge" publishes the caller's selected evidence and changes
    only that caller's branch/worktrees. The membership-based device merge gate is
    removed; the caller still must own the source and branch. No project-wide merge
    status is inferred.
11. The member roster replaces teams and remains visible, including emails, to
    members. Per-member share/revoke, share policies, dismissals, grant-only lists
    and team routes/tools/schema are removed. The console offers Decisions,
    Memories (the caller's live repository memories) and Members. The embedded
    viewer has no project actions and directs users to the web console.
12. Password reset continues to revoke the target's web sessions; password change
    revokes other web sessions. Devices remain independently revocable on Devices.
    Project decisions are fetched online, not delivered through personal sync.
    Personal export is not a project backup: it does not restore project identity,
    membership, publication or governance. Import always creates personal memory.

## 2026-11-17 amendment — committed display name and explicit rename

The committed project file also carries the server-confirmed `name`. Older files
without it remain valid, and init backfills them on a successful join. Normal init
never renames: a different explicit name or a file/server mismatch is an error.
This prevents accidental renames and exposes inconsistent checkout metadata.
Help and unknown flags are resolved locally before any request, because the old
CLI could create a project literally named `--help`.

Only the current owner can rename, using a web session or device token. The
smallest repair surface is `POST projects/{id}/rename` plus
`cairn init --rename <name>` in the initialized checkout; it updates the file as
well as the server without changing identity, membership or decisions. This is
an explicit owner action, with an atomic `project_renamed` audit event. A repeated
same-name rename is a no-op, so retry after a local file-write failure is safe.
PostgreSQL locks the project during rename and during checked init/join, including
against concurrent ownership transfer. No MCP tool or web rename UI is added.
See the [contract](../contracts/project-layer4.md#project-rename) for the full
request, compatibility and recovery rules. Evidence: the client agent-flow,
project-model HTTP, core rename/rollback and PostgreSQL rename regressions.

## Trust and limits

**Possession of a project ID is sufficient to join.** Accounts are created by an
administrator; Cairn does not prove Git repository access. Anyone with an account
on the configured server and a copied or leaked project ID can join permanently,
read every project decision and see the email roster. Removing Git access or the
local file does not revoke membership. No membership removal or account-disable
workflow is added here. Password resets revoke web sessions; device credentials
are revoked separately on Devices.
The self-hosted administrator is trusted for account recovery and owner transfer.

There is no old-data migration. Start development databases and file stores afresh.
Local fallback names can collide across directories and machines; initialized
project decisions still match by project ID. Existing lexical ranking and PostgreSQL's 512-result window remain.

## Defaults — owner may override

- Audit stays private; decisions carry author/changed-by attribution.
- Capture-triggered join remains, and init also joins; failed attachment retries.
- Resume/unresolved stay personal; decisions require online retrieval.
- Live-successor supersession semantics remain; server address stays outside Git.
- Emails remain visible in the member roster; Memories is a live private query.
- CLI/MCP provide a common list, roster, preview/promotion, verify, correct/delete.
- Import is personal only; ownerless legacy projects are unsupported.
- Live eligibility is common to list, recall and maintenance in each store.
- Transfer supports continuity; no account/project deletion workflow is added.

## Evidence

Implementation boundaries: `crates/core/src/projects.rs`, `sharing.rs`,
`recall.rs`, `memories.rs`, `transfer.rs`; `crates/storage/src/lib.rs`;
`crates/server/src/lib.rs`; `crates/client/src/project.rs` and `tools.rs`;
`scripts/cairn_hook.py`; `apps/web/components/project-detail.tsx`.

Focused regressions include
`all_members_read_and_promote_own_decisions_but_only_author_and_owner_edit`,
`recall_ranks_global_repo_and_project_together_without_private_cross_person_rows`,
`promotion_preview_is_full_read_only_and_secret_sources_are_refused`,
`owner_and_admin_transfer_only_to_members_and_keep_authorship_and_private_branches`,
`member_branches_are_personal_and_devices_may_record_merges`,
`remote_rename_keeps_tagged_personal_memories_promotable_by_project_id`, and their PostgreSQL
counterparts in `crates/storage/tests/postgres.rs`. Check results belong in the PR.
