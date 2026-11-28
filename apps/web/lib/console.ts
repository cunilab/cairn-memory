import type { MemoryView } from "@/lib/generated/MemoryView";
import type { PinQuota } from "@/lib/generated/PinQuota";

export type { ConsoleMemory } from "@/lib/generated/ConsoleMemory";
export type { ConsolePage } from "@/lib/generated/ConsolePage";
export type { LiteralMatch as ConsoleMatch } from "@/lib/generated/LiteralMatch";
export type { MatchSegment } from "@/lib/generated/MatchSegment";
export type { PinQuota };

export function quotaFor(memory: Pick<MemoryView, "scope" | "project_id">, quotas: PinQuota[]): PinQuota | undefined {
  if (memory.scope === "personal") return quotas.find((quota) => quota.scope === "personal");
  if (memory.scope === "project" && memory.project_id) {
    return quotas.find((quota) => quota.project_id === memory.project_id);
  }
  return undefined;
}

export function canTogglePin(memory: MemoryView): boolean {
  return memory.can_pin;
}
