// Pure copy for the Activity page: audit rows to sentences. Only a type import, so node:test runs it.

import type { AuditEvent } from "@/lib/generated/AuditEvent";

/**
 * Timeline filter chips; each lists the audit actions it keeps. `all` lists none: the timeline
 * asks for `actor=system`, and the server owns which actions are Cairn's own.
 */
export const TIMELINE_FILTERS = [
  { value: "all", label: "All", actions: [] as readonly string[] },
  { value: "merges", label: "Merges", actions: ["memory_merged"] },
  { value: "links", label: "Links", actions: ["links_added"] },
  { value: "staleness", label: "Staleness", actions: ["repo_state"] },
  { value: "expiry", label: "Expiry", actions: ["memories_expired"] },
  { value: "handoffs", label: "Handoffs", actions: ["session_distilled"] },
] as const;

const AUDIT_LABEL: Record<string, string> = {
  login: "Signed in",
  logout: "Signed out",
  password_changed: "Changed password",
  password_reset: "Reset an account password",
  account_registered: "Registered an account",
  device_registered: "Created a device token",
  device_rotated: "Rotated a device token",
  device_revoked: "Revoked a device token",
  project_initialized: "Registered a project",
  project_renamed: "Renamed a project",
  project_transferred: "Transferred a project",
  agent_instructions_changed: "Changed agent instructions",
  memory_forgotten: "Forgot a memory",
  memories_imported: "Imported memories",
  fix_applied: "Applied a fix",
  fix_ignored: "Ignored a fix",
  fix_done: "Completed a fix",
  fix_failed: "Fix failed",
  fix_copied: "Copied a fix prompt",
  project_auto_joined: "Joined a project automatically",
  memory_rewritten_by_admin: "Rewrote a memory as admin",
};

export function auditLabel(action: string): string {
  if (AUDIT_LABEL[action]) return AUDIT_LABEL[action];
  const text = action.replaceAll("_", " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}
function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}
const plural = (n: number, one: string, many: string) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/** Cairn is the subject, past tense, counts first. Unknown actions stay visible. */
export function describeEvent(event: AuditEvent): string {
  const m = event.metadata ?? {};
  switch (event.action) {
    case "memory_merged": {
      const merged = Array.isArray(m.merged) ? m.merged.length : count(m.count);
      const into = event.titles?.[text(m.into)] ?? text(m.title);
      return `Cairn merged ${plural(merged, "duplicate memory", "duplicate memories")}${into ? ` into "${into}"` : ""}`;
    }
    case "links_added": {
      const entity = text(m.entity);
      const n = count(m.count);
      return `Cairn linked ${plural(n, "memory", "memories")}${entity ? ` that ${n === 1 ? "cites" : "cite"} ${entity}` : ""}`;
    }
    case "memory_promoted": {
      const from = Array.isArray(m.sources) ? m.sources.length : 0;
      return `Cairn shared a memory with everyone${from > 0 ? `, promoted from ${plural(from, "matching memory", "matching memories")}` : ""}`;
    }
    case "memory_withdrawn":
      return "Cairn withdrew a shared memory: fewer than 2 live sources remain";
    case "repo_state": {
      const parts = [
        count(m.stale) > 0 ? `marked ${count(m.stale).toLocaleString()} stale` : "",
        count(m.revived) > 0 ? `revived ${count(m.revived).toLocaleString()}` : "",
        count(m.promoted) > 0 ? `moved ${count(m.promoted).toLocaleString()} from merged branches` : "",
      ].filter(Boolean);
      return parts.length > 0 ? `Cairn ${parts.join(", ")}` : "Cairn checked memories against the repository";
    }
    case "memories_expired": {
      const by = m.by_kind && typeof m.by_kind === "object" ? Object.entries(m.by_kind as Record<string, unknown>) : [];
      const detail = by.map(([kind, n]) => `${count(n)} ${kind}`).join(", ");
      return `Cairn expired ${plural(count(m.count), "memory", "memories")}${detail ? ` (${detail})` : ""}`;
    }
    case "session_distilled": {
      const branch = text(m.branch);
      const notes = count(m.notes);
      return `Cairn wrote a handoff${branch ? ` for ${branch}` : ""}${notes > 0 ? ` and saved ${plural(notes, "note", "notes")}` : ""}`;
    }
    case "job_quarantined": {
      const error = text(m.error);
      return `A ${text(m.kind) || "background"} job stopped after ${plural(count(m.attempts), "try", "tries")}${error ? `: ${error}` : ""}`;
    }
    default:
      return `Cairn: ${event.action}`;
  }
}

/** Memories an event touched, for the inline expand (titles the reader may still see). */
export function touchedMemories(event: AuditEvent): { id: string; title: string }[] {
  return Object.entries(event.titles ?? {}).flatMap(([id, title]) => (title ? [{ id, title }] : []));
}

/** Local calendar day of an epoch-seconds stamp, `YYYY-MM-DD`. */
export function dayKey(seconds: number): string {
  const d = new Date(seconds * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** "Today", "Yesterday" or the date, relative to `nowSeconds`. */
export function dayLabel(key: string, nowSeconds: number): string {
  if (key === dayKey(nowSeconds)) return "Today";
  if (key === dayKey(nowSeconds - 86400)) return "Yesterday";
  const [y, mo, d] = key.split("-").map(Number);
  return new Date(y, mo - 1, d).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}
