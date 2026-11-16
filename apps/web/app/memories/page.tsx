"use client";

import { useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";
import type { ProjectMemory } from "@/lib/types";

const scopeLabel = { project: "Project", personal: "Personal behaviour", global: "Global" };

export default function MemoriesPage() {
  const router = useRouter();
  const [memories, setMemories] = useState<ProjectMemory[]>([]);
  const [detail, setDetail] = useState<ProjectMemory | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  useResetOnHide(() => { setMemories([]); setDetail(null); setError(null); });
  useEffect(() => {
    let cancelled = false;
    cairn<ProjectMemory[]>("memories")
      .then((rows) => { if (!cancelled) setMemories(rows); })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) router.replace("/");
        else setError(cause instanceof Error ? cause.message : "Could not load memories");
      })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [router]);
  const columns = useMemo<ColumnDef<ProjectMemory>[]>(() => [
    {
      id: "memory", header: "Memory", accessorFn: (m) => `${m.title} ${m.summary}`,
      cell: ({ row }) => <div className="flex min-w-0 flex-col gap-1.5">
        <button type="button" aria-haspopup="dialog" className="text-left text-sm font-medium break-words hover:underline focus-visible:ring-2" onClick={() => setDetail(row.original)}>{row.original.title}</button>
        <p className="line-clamp-3 text-sm text-muted-foreground">{row.original.summary}</p>
      </div>,
    },
    { accessorKey: "scope", header: "Scope", cell: ({ row }) => scopeLabel[row.original.scope] },
    { accessorKey: "kind", header: "Type" },
  ], []);
  return <ConsoleShell crumbs={[{ label: "Memories" }]}>
    <PageHeader title="Memories" description="Project knowledge, your private behaviour preferences, and global lessons shared across this server." />
    {error ? <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert> : null}
    <DataTable label="memories" data={memories} columns={columns} getRowId={(m) => m.memory_id} filterPlaceholder="Filter memories" empty={<p className="p-6 text-sm text-muted-foreground">{loading ? "Loading memories…" : "No memories yet."}</p>} />
    <Sheet open={detail !== null} onOpenChange={(open) => !open && setDetail(null)}>
      <SheetContent className="overflow-y-auto">
        <SheetHeader><SheetTitle>{detail?.title ?? "Memory"}</SheetTitle><SheetDescription>{detail ? scopeLabel[detail.scope] : "Memory fields"}</SheetDescription></SheetHeader>
        <div className="space-y-4 p-4">{detail ? Object.entries(detail).map(([field, value]) => <div key={field}>
          <p className="text-xs font-medium text-muted-foreground">{field.replaceAll("_", " ")}</p>
          <p className="whitespace-pre-wrap break-words text-sm">{typeof value === "object" && value !== null ? JSON.stringify(value, null, 2) : String(value ?? "—")}</p>
        </div>) : null}</div>
      </SheetContent>
    </Sheet>
  </ConsoleShell>;
}
