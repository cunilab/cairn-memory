"use client";

import { useMemo } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable } from "@/components/data-table";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import type { AuditEvent } from "@/lib/generated/AuditEvent";
import { auditLabel } from "../_lib/events";
import { useAudit } from "../_lib/use-audit";

export function AuditTable() {
  const { events, error, retry, hasMore, loadingMore, loadOlder } = useAudit("person");

  const columns = useMemo<ColumnDef<AuditEvent>[]>(
    () => [
      {
        accessorKey: "created_at",
        header: "Time",
        cell: ({ row }) => <Time value={row.original.created_at} className="text-xs whitespace-nowrap text-muted-foreground" />,
      },
      { id: "action", header: "Action", accessorFn: (e) => auditLabel(e.action) },
      {
        id: "subject",
        header: "Subject",
        accessorFn: (e) => e.subject_id ?? "",
        cell: ({ row }) =>
          row.original.subject_id ? (
            <code className="font-mono text-xs break-all text-muted-foreground">{row.original.subject_id}</code>
          ) : (
            <span className="text-muted-foreground">—</span>
          ),
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "device",
        header: "Device",
        accessorFn: (e) => e.device_name ?? "",
        cell: ({ row }) => row.original.device_name ?? <span className="text-muted-foreground">—</span>,
        meta: { className: "hidden md:table-cell" },
      },
    ],
    [],
  );

  if (error) return <ErrorAlert message={error} onRetry={retry} />;
  if (events === null) return <LoadingBlock label="Loading audit log…" rows={6} />;
  return (
    <div className="flex flex-col gap-3">
      <DataTable
        label="audit log"
        data={events}
        columns={columns}
        getRowId={(e) => e.audit_id}
        filterPlaceholder="Filter audit log"
        initialSort={[{ id: "created_at", desc: true }]}
        pageSize={20}
        empty={<p className="p-6 text-sm text-muted-foreground">No sign-ins or changes recorded yet.</p>}
      />
      {hasMore ? (
        <div className="flex justify-center">
          <Button type="button" variant="outline" disabled={loadingMore} onClick={() => void loadOlder()}>
            {loadingMore ? "Loading…" : "Load older"}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
