"use client";

import { useMemo } from "react";
import Link from "next/link";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable } from "@/components/data-table";
import { Time } from "@/components/time";
import { Badge } from "@/components/ui/badge";
import { useAccount } from "@/hooks/use-account";
import type { SessionReport } from "@/lib/generated/SessionReport";
import { sessionTitle } from "@/lib/provenance";
import {
  formatWhyNone,
  hasSaves,
  renderOutcomeStatus,
  summaryLine,
} from "../_lib/session-summary";

export function ProjectSessions({ sessions, initialFilter = "" }: { sessions: SessionReport[]; initialFilter?: string }) {
  const account = useAccount();
  const withSaves = useMemo(
    () => sessions.filter(hasSaves).length,
    [sessions],
  );

  const columns = useMemo<ColumnDef<SessionReport>[]>(
    () => [
      {
        accessorKey: "started_at",
        header: "Started",
        cell: ({ row }) => (
          <Time
            value={row.original.started_at}
            className="text-xs whitespace-nowrap text-muted-foreground"
          />
        ),
      },
      {
        id: "branch",
        header: "Branch",
        accessorFn: (s) => s.branch ?? "",
        cell: ({ row }) => (
          <div className="flex items-center gap-1.5">
            <code className="font-mono text-xs break-all">
              {row.original.branch ?? "—"}
            </code>
            {row.original.automated ? (
              <Badge variant="outline">auto</Badge>
            ) : null}
          </div>
        ),
      },
      {
        accessorKey: "files_changed",
        header: "Files",
        cell: ({ row }) => (
          <span className="tabular-nums">
            {row.original.files_changed !== null ? row.original.files_changed : "—"}
          </span>
        ),
      },
      {
        accessorKey: "notes_written",
        header: "Notes",
        cell: ({ row }) => (
          <span className="tabular-nums">
            {row.original.notes_written !== null ? row.original.notes_written : "—"}
          </span>
        ),
      },
      {
        id: "digest",
        header: "Digest",
        cell: ({ row }) => {
          const status = renderOutcomeStatus(row.original);
          if (status) return <Badge variant="outline">{status}</Badge>;
          if (row.original.digest_written) {
            return <Badge variant="brand">written</Badge>;
          }
          return <span className="text-muted-foreground">—</span>;
        },
      },
      {
        id: "handoff",
        header: "Handoff",
        cell: ({ row }) => {
          const status = renderOutcomeStatus(row.original);
          if (status) return <Badge variant="outline">{status}</Badge>;
          const outcome = row.original.handoff_outcome;
          if (!outcome || outcome === "none") {
            return <span className="text-muted-foreground">—</span>;
          }
          return (
            <Badge variant={outcome === "written" ? "brand" : "outline"}>
              {outcome}
            </Badge>
          );
        },
      },
      {
        accessorKey: "agent_saves",
        header: "New records in this time window",
        cell: ({ row }) => (
          <span className="tabular-nums">{row.original.agent_saves}</span>
        ),
      },
      {
        id: "open",
        header: "Inspector",
        cell: ({ row }) => {
          const session = row.original;
          if (session.chat && session.person_id !== account?.person_id) {
            return <span className="text-muted-foreground">—</span>;
          }
          const query = new URLSearchParams({ person: session.person_id, session: session.session_id });
          return (
            <Link className="text-sm underline-offset-4 hover:underline" href={`/session?${query}`}>
              {sessionTitle(session.branch)}
            </Link>
          );
        },
      },
      {
        id: "why_nothing",
        header: "Why nothing",
        cell: ({ row }) => {
          const status = renderOutcomeStatus(row.original);
          if (status) return <Badge variant="outline">{status}</Badge>;
          if (hasSaves(row.original)) {
            return <span className="text-muted-foreground">—</span>;
          }
          return (
            <span className="text-xs text-muted-foreground">
              {formatWhyNone(row.original.why_none)}
            </span>
          );
        },
      },
    ],
    [account?.person_id],
  );

  return (
    <div className="flex flex-col gap-3">
      <div className="text-sm text-muted-foreground">
        <p className="font-medium">{summaryLine(sessions.length, withSaves)}</p>
        <p>Counts are new records during each session&apos;s time window.</p>
      </div>
      <DataTable
        label="project sessions"
        data={sessions}
        columns={columns}
        getRowId={(s) => s.session_id}
        initialFilter={initialFilter}
        filterPlaceholder="Filter sessions"
        empty={
          <p className="p-6 text-sm text-muted-foreground">
            No sessions recorded for this project yet.
          </p>
        }
      />
    </div>
  );
}
