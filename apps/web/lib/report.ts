/** The server stamps records with epoch seconds. */
export function formatTime(value: string | number | null | undefined): string {
  const seconds = Number(value);
  if (value === null || value === undefined || value === "" || !Number.isFinite(seconds)) {
    return value ? String(value) : "—";
  }
  return new Date(seconds * 1000).toLocaleString();
}

const UNITS: [limit: number, size: number, name: string][] = [
  [60, 1, "s"],
  [3600, 60, "min"],
  [86400, 3600, "h"],
  [86400 * 60, 86400, "d"],
];

/** "3 min ago", "2 d ago"; older than 60 days falls back to the date. */
export function relativeTime(seconds: number, nowSeconds: number): string {
  const diff = Math.max(0, nowSeconds - seconds);
  if (diff < 45) return "just now";
  for (const [limit, size, name] of UNITS) {
    if (diff < limit) return `${Math.max(1, Math.round(diff / size))} ${name} ago`;
  }
  return new Date(seconds * 1000).toLocaleDateString();
}
