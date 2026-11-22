"use client";

import { useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable } from "@/components/data-table";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { cairn } from "@/lib/cairn";
import type { Device } from "@/lib/generated/Device";
import type { AuditEvent } from "@/lib/generated/AuditEvent";
import { auditLabel, isSystemAction } from "../_lib/events";
import { useAudit } from "../_lib/use-audit";

export function AuditTable() {
  const { events, error, retry } = useAudit();
  const [devices, setDevices] = useState<Map<string, string>>(new Map());

  // TODO(7a): `AuditEvent.device_name` replaces this lookup.
  useEffect(() => {
    let cancelled = false;
    cairn<Device[]>("devices")
      .then((rows) => !cancelled && setDevices(new Map(rows.map((d) => [d.device_id, d.name]))))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const rows = useMemo(() => (events ?? []).filter((event) => !isSystemAction(event.action)), [events]);
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
        accessorFn: (e) => (e.device_id ? (devices.get(e.device_id) ?? e.device_id) : ""),
        cell: ({ row }) => {
          const id = row.original.device_id;
          return id ? (devices.get(id) ?? id) : <span className="text-muted-foreground">—</span>;
        },
        meta: { className: "hidden md:table-cell" },
      },
    ],
    [devices],
  );

  if (error) return <ErrorAlert message={error} onRetry={retry} />;
  if (events === null) return <LoadingBlock label="Loading audit log…" rows={6} />;
  return (
    <DataTable
      label="audit log"
      data={rows}
      columns={columns}
      getRowId={(e) => e.audit_id}
      filterPlaceholder="Filter audit log"
      initialSort={[{ id: "created_at", desc: true }]}
      pageSize={20}
      empty={<p className="p-6 text-sm text-muted-foreground">No sign-ins or changes recorded yet.</p>}
    />
  );
}
