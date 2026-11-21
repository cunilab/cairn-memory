"use client";

import { useMemo, useState } from "react";
import { ChevronRightIcon, WorkflowIcon } from "lucide-react";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { useOpenMemory, useQueryParams } from "@/hooks/use-query-params";
import { cn } from "@/lib/utils";
import {
  dayKey,
  dayLabel,
  describeEvent,
  isSystemAction,
  TIMELINE_FILTERS,
  touchedMemories,
  type AuditEvent,
} from "../_lib/events";
import { useAudit } from "../_lib/use-audit";

function Row({ event }: { event: AuditEvent }) {
  const [open, setOpen] = useState(false);
  const openMemory = useOpenMemory();
  const touched = touchedMemories(event);
  const date = new Date(event.created_at * 1000);
  return (
    <li className="flex flex-col gap-1 py-2">
      <div className="flex items-start gap-3">
        <time
          dateTime={date.toISOString()}
          title={date.toLocaleString()}
          className="w-12 shrink-0 pt-0.5 text-xs text-muted-foreground tabular-nums"
        >
          {date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
        </time>
        <p className="min-w-0 flex-1 text-sm [overflow-wrap:anywhere]">{describeEvent(event)}</p>
        {touched.length > 0 ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            aria-expanded={open}
            aria-label={`${open ? "Hide" : "Show"} ${touched.length} memories`}
            onClick={() => setOpen((value) => !value)}
          >
            {touched.length}
            <ChevronRightIcon className={cn("transition-transform", open && "rotate-90")} />
          </Button>
        ) : null}
      </div>
      {open ? (
        <ul className="ml-15 flex flex-col gap-1 text-sm">
          {touched.map((memory) => (
            <li key={memory.id}>
              <button
                type="button"
                className="rounded-sm text-left underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                onClick={() => openMemory(memory.id)}
              >
                {memory.title}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}

export function Timeline() {
  const { events, error, retry } = useAudit();
  const [params, set] = useQueryParams();
  const [now] = useState(() => Math.floor(Date.now() / 1000));
  const filter = TIMELINE_FILTERS.find((item) => item.value === params.get("type")) ?? TIMELINE_FILTERS[0];

  const days = useMemo(() => {
    const kept = (events ?? []).filter(
      (event) => isSystemAction(event.action) && (filter.actions as readonly string[]).includes(event.action),
    );
    const groups = new Map<string, AuditEvent[]>();
    for (const event of kept) {
      const key = dayKey(event.created_at);
      groups.set(key, [...(groups.get(key) ?? []), event]);
    }
    return [...groups];
  }, [events, filter]);

  if (error) return <ErrorAlert message={error} onRetry={retry} />;
  if (events === null) return <LoadingBlock label="Loading timeline…" rows={6} />;

  return (
    <div className="flex flex-col gap-4">
      <div role="group" aria-label="Filter timeline" className="flex flex-wrap gap-2">
        {TIMELINE_FILTERS.map((item) => (
          <Button
            key={item.value}
            type="button"
            size="sm"
            variant={item.value === filter.value ? "default" : "outline"}
            aria-pressed={item.value === filter.value}
            onClick={() => set({ type: item.value === "all" ? null : item.value })}
          >
            {item.label}
          </Button>
        ))}
      </div>
      {days.length === 0 ? (
        <Empty className="border">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <WorkflowIcon />
            </EmptyMedia>
            <EmptyTitle>Nothing automatic yet</EmptyTitle>
            <EmptyDescription>
              Cairn starts merging, linking and checking memories once your agents save some.
            </EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        days.map(([key, items]) => (
          <section key={key} aria-labelledby={`day-${key}`}>
            <h2 id={`day-${key}`} className="text-sm font-semibold">
              {dayLabel(key, now)}
            </h2>
            <ol className="divide-y">
              {items.map((event) => (
                <Row key={event.audit_id} event={event} />
              ))}
            </ol>
          </section>
        ))
      )}
      {/* TODO(7a): "Load older" pages with `before` once the audit endpoint supports it. */}
    </div>
  );
}
