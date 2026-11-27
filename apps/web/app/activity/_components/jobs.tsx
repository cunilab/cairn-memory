"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable } from "@/components/data-table";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Badge } from "@/components/ui/badge";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { loginHref, SessionRequiredError } from "@/lib/session";
import type { Job } from "@/lib/generated/Job";
import type { JobCounts } from "@/lib/generated/JobCounts";
import type { JobSnapshot } from "@/lib/generated/JobSnapshot";
import type { Attention } from "@/lib/generated/Attention";

/** Admins get the rows; everyone else gets their own counts from `/stats`. */
type Queue = JobCounts & { quarantine?: Job[] };

/** Read-only: Cairn retries failed jobs by itself, so there is nothing for a person to press. */
export function Jobs() {
  const router = useRouter();
  const account = useAccount();
  const admin = account?.is_admin === true;
  const [queue, setQueue] = useState<Queue | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useResetOnHide(() => {
    setQueue(null);
    setError(null);
  });

  useEffect(() => {
    if (account === undefined) return;
    let cancelled = false;
    (admin ? cairn<JobSnapshot>("admin/jobs") : cairn<Attention>("stats/attention").then((next) => next.jobs))
      .then((next) => {
        if (cancelled) return;
        setQueue(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(loginHref());
          return;
        }
        setError(`Jobs could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
      });
    return () => {
      cancelled = true;
    };
  }, [account, admin, attempt, router]);

  const retry = useCallback(() => {
    setQueue(null);
    setError(null);
    setAttempt((n) => n + 1);
  }, []);

  const columns = useMemo<ColumnDef<Job>[]>(
    () => [
      {
        accessorKey: "kind",
        header: "Job",
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="font-medium break-words">{row.original.kind}</span>
            <code className="font-mono text-xs break-all text-muted-foreground">{row.original.key}</code>
          </div>
        ),
      },
      {
        accessorKey: "status",
        header: "Status",
        cell: () => <Badge variant="destructive">Stopped retrying</Badge>,
      },
      {
        accessorKey: "attempts",
        header: "Attempts",
        cell: ({ row }) => <span className="tabular-nums">{row.original.attempts}</span>,
        meta: { className: "hidden sm:table-cell" },
      },
      {
        accessorKey: "last_error",
        header: "Last error",
        cell: ({ row }) => row.original.last_error ? (
          <details>
            <summary className="cursor-pointer text-muted-foreground">Show error and attempts</summary>
            <p className="mt-1 max-w-md text-sm break-words">{row.original.last_error}</p>
            <p className="text-xs tabular-nums text-muted-foreground">{row.original.attempts} attempts</p>
          </details>
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        cell: ({ row }) => <Time value={row.original.updated_at} className="text-xs whitespace-nowrap text-muted-foreground" />,
        meta: { className: "hidden md:table-cell" },
      },
    ],
    [],
  );

  if (account === undefined) return <LoadingBlock label="Loading jobs…" rows={3} />;
  if (error) return <ErrorAlert message={error} onRetry={retry} />;
  if (queue === null) return <LoadingBlock label="Loading jobs…" rows={4} />;

  return (
    <div className="flex flex-col gap-4">
      <dl className="grid gap-3 sm:grid-cols-3">
        <QueueCount label="Pending" value={queue.pending} />
        <QueueCount label="Running" value={queue.running} />
        <QueueCount label="Stopped retrying" value={queue.quarantined} />
      </dl>
      {queue.quarantine ? (
      <DataTable
        columns={columns}
        data={queue.quarantine}
        label="stopped jobs"
        filterPlaceholder="Filter stopped jobs"
        getRowId={(row) => row.job_id}
        empty={
          <Empty className="border-0">
            <EmptyHeader>
              <EmptyTitle>No jobs waiting</EmptyTitle>
              <EmptyDescription>Everything is processed.</EmptyDescription>
            </EmptyHeader>
          </Empty>
        }
      />
      ) : null}
      <p className="text-sm text-muted-foreground">
        Cairn retries failed jobs automatically. A job that fails 5 times stops, and is tried again after the next server update and after 24 hours.
      </p>
    </div>
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
