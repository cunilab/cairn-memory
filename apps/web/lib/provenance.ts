import type { MemoryProvenance } from "@/lib/generated/MemoryProvenance";

/** Link to the session inspector. The label is separate so a missing branch is not invented. */
export function sessionHref(provenance: Pick<MemoryProvenance, "person_id" | "session_id">): string {
  const query = new URLSearchParams({
    person: provenance.person_id,
    session: provenance.session_id,
  });
  return `/session?${query}`;
}

export function sessionTitle(branch: string | null): string {
  return branch ? `Session on ${branch}` : "Session";
}

/** The project's Sessions tab. A branch prefills the table filter and is not a session citation. */
export function branchActivityHref(projectId: string, branch: string | null): string {
  const query = new URLSearchParams({ tab: "sessions" });
  if (branch) query.set("branch", branch);
  return `/projects/${encodeURIComponent(projectId)}?${query}`;
}
