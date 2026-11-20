/** The server stamps records with epoch seconds. */
export function formatTime(value: string | number | null | undefined): string {
  const seconds = Number(value);
  if (value === null || value === undefined || value === "" || !Number.isFinite(seconds)) {
    return value ? String(value) : "—";
  }
  return new Date(seconds * 1000).toLocaleString();
}

export function newestFirst<T extends { created_at?: number }>(items: T[]): T[] {
  return [...items].sort((a, b) => (b.created_at ?? 0) - (a.created_at ?? 0));
}
