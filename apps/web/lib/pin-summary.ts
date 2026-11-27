import type { PinSummary } from "@/lib/generated/PinSummary";

export function pinFitText(summary: Pick<PinSummary, "shown" | "total" | "not_shown">): {
  fit: string | null;
  overflow: string | null;
} {
  if (summary.total === 0) {
    return { fit: null, overflow: null };
  }
  return {
    fit: `${summary.shown} of ${summary.total} pins fit in the brief`,
    overflow: summary.not_shown > 0 ? `${summary.not_shown} more pinned not shown` : null,
  };
}
