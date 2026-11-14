/** The server stamps records with epoch seconds as a string. */
export function formatTime(value: string | null | undefined): string {
  const seconds = Number(value);
  if (!value || !Number.isFinite(seconds)) {
    return value || "—";
  }
  return new Date(seconds * 1000).toLocaleString();
}

export function newestFirst<T extends { created_at?: string }>(items: T[]): T[] {
  return [...items].sort((a, b) => Number(b.created_at ?? 0) - Number(a.created_at ?? 0));
}
