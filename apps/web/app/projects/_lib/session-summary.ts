/** Summary line above the sessions table: "N sessions, M with saves" */
export function summaryLine(total: number, withSaves: number): string {
  return `${total} sessions, ${withSaves} with saves`;
}

/** Whether the session has saves (notes created during distill or saves during the session). */
export function hasSaves(session: { notes_written?: number | null; agent_saves?: number }): boolean {
  return (session.notes_written ?? 0) > 0 || (session.agent_saves ?? 0) > 0;
}

/**
 * Returns "before 0.1.9" when distilled_at is null,
 * "open" when distilled_at is set but ended_at is null,
 * or null when the session ended normally.
 */
export function renderOutcomeStatus(session: {
  distilled_at?: number | null;
  ended_at?: number | null;
}): "before 0.1.9" | "open" | null {
  if (session.distilled_at === null || session.distilled_at === undefined) {
    return "before 0.1.9";
  }
  if (session.ended_at === null || session.ended_at === undefined) {
    return "open";
  }
  return null;
}

const WHY_NONE_LABELS: Record<string, string> = {
  no_edits: "no edits",
  no_typed_lines: "no decision lines",
  no_test_runs: "no test runs",
  no_fixed_failures: "no fixed failures",
  all_duplicates: "all duplicates",
  automated: "automated",
};

/** Formats why_none codes into human-readable label string. */
export function formatWhyNone(codes: string[] | null | undefined): string {
  if (!codes || codes.length === 0) return "—";
  return codes.map((code) => WHY_NONE_LABELS[code] ?? code.replaceAll("_", " ")).join(", ");
}
