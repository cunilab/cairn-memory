// Pure helpers for the dashboard copy. No imports, so node:test runs them.

/** Server-side usage report (step 7a). */
export interface UsageDays {
  days: { day: string; tokens: number; lookups: number; hits: number }[];
  total_tokens: number;
  lookups: number;
  hit_ratio: number;
}

/** One generated sentence of value: numbers only, estimates marked with "~". */
export function valueSentence(usage: UsageDays | undefined, days = 30): string {
  if (!usage || usage.lookups === 0) {
    return "No agent lookups yet. Savings appear after your agents search memory.";
  }
  const found = Math.round(usage.hit_ratio * 100);
  return `Your agents made ${usage.lookups.toLocaleString()} memory lookups in the last ${days} days; ${found}% found something relevant. Cairn delivered ~${usage.total_tokens.toLocaleString()} tokens of memory context.`;
}

export type Segment = { key: string; label: string; count: number };

const STATUS_LABEL: [string, string][] = [
  ["active", "Active"],
  ["stale", "Stale"],
  ["superseded", "Superseded"],
];

/** Health bar segments in a fixed order; zero segments are dropped. */
export function healthSegments(byStatus: Record<string, number>): Segment[] {
  return STATUS_LABEL.map(([key, label]) => ({ key, label, count: byStatus[key] ?? 0 })).filter((s) => s.count > 0);
}

/** The text the stacked bar announces: every count, so colour is never the only cue. */
export function healthLabel(segments: Segment[]): string {
  return segments.length === 0
    ? "No memories yet"
    : segments.map((s) => `${s.label} ${s.count.toLocaleString()}`).join(", ");
}
