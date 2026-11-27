"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { Pin } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { onMemoriesChanged } from "@/lib/memory-events";
import { pinFitText } from "@/lib/pin-summary";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { PinSummary } from "@/lib/generated/PinSummary";
import { cn } from "@/lib/utils";

type Props = {
  projectId?: string | null;
  memoryId?: string;
  pinnedAt?: number | null;
  compact?: boolean;
  className?: string;
};

export function PinSummaryBar({ projectId, memoryId, pinnedAt, compact = false, className }: Props) {
  const router = useRouter();
  const [summary, setSummary] = useState<PinSummary | null>(null);
  const [attempt, setAttempt] = useState(0);
  const query = projectId ? `memories/pins?projectId=${encodeURIComponent(projectId)}` : "memories/pins";

  useResetOnHide(() => setSummary(null));

  useEffect(() => onMemoriesChanged(() => setAttempt((n) => n + 1)), []);

  useEffect(() => {
    let cancelled = false;
    cairn<PinSummary>(query)
      .then((next) => {
        if (!cancelled) setSummary(next);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(LOGIN_PATH);
          return;
        }
        setSummary(null);
      });
    return () => {
      cancelled = true;
    };
  }, [query, attempt, pinnedAt, router]);

  if (!summary) return null;

  const { fit, overflow } = pinFitText(summary);
  const inBrief = memoryId ? summary.shown_ids.includes(memoryId) : false;
  const omittedFromBrief = memoryId ? summary.omitted_ids.includes(memoryId) : false;

  return (
    <div
      data-slot="pin-summary"
      className={cn(
        "flex flex-wrap items-center gap-2 text-xs text-muted-foreground",
        compact ? "" : "rounded-md border bg-muted/30 px-3 py-2",
        className,
      )}
    >
      <span className="inline-flex items-center gap-1 font-medium text-foreground">
        <Pin aria-hidden="true" className="size-3.5 text-muted-foreground" />
        Pins
      </span>
      <Badge variant="outline" className="tabular-nums">
        {summary.personal_quota_used}/{summary.personal_quota_limit} personal
      </Badge>
      {summary.project_quota_used !== null ? (
        <Badge variant="outline" className="tabular-nums">
          {summary.project_quota_used}/{summary.project_quota_limit} project
        </Badge>
      ) : null}
      {fit ? <span className="tabular-nums">{fit}</span> : null}
      {overflow ? <Badge variant="brand" className="tabular-nums">{overflow}</Badge> : null}
      {pinnedAt != null && omittedFromBrief ? (
        <span className="text-foreground">Not shown in L0 brief</span>
      ) : pinnedAt != null && inBrief ? (
        <span>Shown in L0 brief</span>
      ) : null}
    </div>
  );
}
