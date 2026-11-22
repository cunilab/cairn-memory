"use client";

import { useState } from "react";
import { relativeTime } from "@/lib/report";

/** Relative time with the absolute stamp in the tooltip. Only render it once data has loaded. */
export function Time({ value, className }: { value: number | null | undefined; className?: string }) {
  // Read once per mount: the relative text is a snapshot, not a ticking clock.
  const [now] = useState(() => Math.floor(Date.now() / 1000));
  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">—</span>;
  }
  const date = new Date(value * 1000);
  return (
    <time dateTime={date.toISOString()} title={date.toLocaleString()} className={className}>
      {relativeTime(value, now)}
    </time>
  );
}
