/** What the device row said when its token was issued; a check-in is anything newer. */
export type Baseline = { hook: number | null; seen: number };
type Row = { last_hook_at: number | null; last_seen_at: number };

export const baselineOf = (device: Row): Baseline => ({ hook: device.last_hook_at, seen: device.last_seen_at });

/** Rotation keeps `last_hook_at`, so "has a hook time" would call a rotated device connected at once. */
export function checkedInSince(row: Row, baseline: Baseline): boolean {
  return (row.last_hook_at !== null && row.last_hook_at > (baseline.hook ?? 0)) || row.last_seen_at > baseline.seen;
}
