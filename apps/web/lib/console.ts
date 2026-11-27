import type { MemoryView } from "@/lib/generated/MemoryView";

/** One highlighted piece of a literal match. Mirrors the console browse JSON. */
export type MatchSegment = { text: string; hit: boolean };

export type ConsoleMatch = { field: string; label: string; segments: MatchSegment[] };

export type PinQuota = {
  scope: string;
  project_id: string | null;
  name: string;
  used: number;
  limit: number;
};

export type ConsoleMemory = MemoryView & { matched?: ConsoleMatch };

export type ConsolePage = {
  items: ConsoleMemory[];
  page: number;
  page_size: number;
  total: number;
  quotas: PinQuota[];
};

export function quotaFor(memory: Pick<MemoryView, "scope" | "project_id">, quotas: PinQuota[]): PinQuota | undefined {
  if (memory.scope === "personal") return quotas.find((quota) => quota.scope === "personal");
  if (memory.scope === "project" && memory.project_id) {
    return quotas.find((quota) => quota.project_id === memory.project_id);
  }
  return undefined;
}

export function canTogglePin(memory: MemoryView): boolean {
  if (!memory.owner_is_me || memory.project_archived || memory.scope === "global" || memory.status === "forgotten") {
    return false;
  }
  return memory.status !== "superseded" || memory.pinned_at != null;
}
