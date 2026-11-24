/** A link the memory list already has. Order is oldest first. */
export type LabelLink = {
  relation: string;
  source: string;
  target: string;
};

export type MemoryLabel = {
  text: string;
  /** The replacement, when the label is `replaced by`. */
  id: string | null;
};

/** The status-slot label. Active cards and unexplained rows have none. */
export function memoryLabel(memory: {
  id: string;
  status: string;
  stale_cause?: string | null;
  wrong?: number;
  links: LabelLink[];
}): MemoryLabel | null {
  if (memory.status === "superseded") {
    const incoming = memory.links.filter(
      (link) => link.relation === "supersedes" && link.target === memory.id,
    );
    const newest = incoming.at(-1);
    if (!newest) return null;
    return { text: `replaced by ${newest.source}`, id: newest.source };
  }
  if (memory.status === "stale" && memory.stale_cause === "feedback") {
    return { text: `voted wrong ${memory.wrong ?? 0}x`, id: null };
  }
  if (memory.status === "stale" && memory.stale_cause === "missing") {
    return { text: "file deleted", id: null };
  }
  return null;
}
