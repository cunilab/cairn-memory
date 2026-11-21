"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { ListTodoIcon } from "lucide-react";
import { toast } from "sonner";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { useAccount } from "@/hooks/use-account";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { JobQueue, QuarantinedJob } from "@/lib/types";

export function Jobs() {
  const router = useRouter();
  const account = useAccount();
  const admin = account?.is_admin === true;
  const [queue, setQueue] = useState<JobQueue | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setQueue(await cairn<JobQueue>("admin/jobs"));
      setError(null);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace("/");
        return;
      }
      setError(cause instanceof Error ? cause.message : "Could not load jobs");
    }
  }, [router]);

  useEffect(() => {
    if (admin) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      void load();
    }
  }, [admin, load]);

  const act = useCallback(
    async (job: QuarantinedJob, action: "requeue" | "discard") => {
      setBusy(job.job_id + action);
      try {
        await cairn(`admin/jobs/${encodeURIComponent(job.job_id)}/${action}`, { method: "POST" });
        toast.success(action === "requeue" ? "Job requeued" : "Job discarded");
        await load();
      } catch (cause) {
        toast.error(cause instanceof Error ? cause.message : "Could not update the job");
      } finally {
        setBusy(null);
      }
    },
    [load],
  );

  const columns = useMemo<ColumnDef<QuarantinedJob>[]>(
    () => [
      {
        accessorKey: "kind",
        header: "Job",
        cell: ({ row }) => (
          <div className="min-w-0">
            <div className="font-medium">{row.original.kind} · {row.original.key}</div>
            <div className="text-xs text-muted-foreground">
              {row.original.attempts} {row.original.attempts === 1 ? "attempt" : "attempts"} ·{" "}
              {formatTime(row.original.updated_at)}
            </div>
          </div>
        ),
      },
      {
        accessorKey: "last_error",
        header: "Last error",
        meta: { className: "hidden md:table-cell" },
        cell: ({ row }) => (
          <span className="block max-w-md truncate text-muted-foreground" title={row.original.last_error ?? undefined}>
            {row.original.last_error}
          </span>
        ),
      },
      {
        id: "actions",
        header: "",
        cell: ({ row }) => (
          <div className="flex justify-end gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={busy !== null}
              onClick={() => void act(row.original, "requeue")}
            >
              Requeue
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy !== null}
              onClick={() => void act(row.original, "discard")}
            >
              Discard
            </Button>
          </div>
        ),
      },
    ],
    [act, busy],
  );

  return (
    <ConsoleShell crumbs={[{ label: "Jobs" }]}>
      <PageHeader
        title="Jobs"
        description="Queue depth for embedding, linking, consolidation and session distillation. Quarantined jobs are retried automatically after an hour."
      />
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      {!admin ? (
        <Empty>
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <ListTodoIcon />
            </EmptyMedia>
            <EmptyTitle>Admin only</EmptyTitle>
            <EmptyDescription>Only an admin can inspect background jobs.</EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : queue === null && !error ? (
        <Skeleton className="h-40 w-full" />
      ) : queue ? (
        <div className="flex flex-col gap-6">
          <dl className="grid gap-3 sm:grid-cols-3">
            <QueueCount label="Pending" value={queue.pending} />
            <QueueCount label="Running" value={queue.running} />
            <QueueCount label="Quarantined" value={queue.quarantined} />
          </dl>
          <DataTable
            columns={columns}
            data={queue.quarantine}
            label="quarantined jobs"
            filterPlaceholder="Filter quarantined jobs"
            getRowId={(row) => row.job_id}
            empty={
              <Empty>
                <EmptyHeader>
                  <EmptyTitle>No quarantined jobs</EmptyTitle>
                  <EmptyDescription>Failed jobs that will not retry appear here.</EmptyDescription>
                </EmptyHeader>
              </Empty>
            }
          />
        </div>
      ) : null}
    </ConsoleShell>
  );
}

function QueueCount({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg border px-4 py-3">
      <dt className="text-sm text-muted-foreground">{label}</dt>
      <dd className="text-2xl font-semibold tabular-nums">{value}</dd>
    </div>
  );
}
