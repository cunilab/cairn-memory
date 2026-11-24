"use client";

import { useCallback, useEffect, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { toast } from "sonner";
import { DataTable } from "@/components/data-table";
import { Button } from "@/components/ui/button";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useAccount } from "@/hooks/use-account";
import { useQueryParams, useTabParam } from "@/hooks/use-query-params";
import { cairn } from "@/lib/cairn";
import { buttonsFor } from "../_lib/fixes-buttons";

type Fix = Parameters<typeof buttonsFor>[0] & { fix_id: string; detector: string; fingerprint: string; subject_id: string | null; project_name: string | null; device_id: string | null; device_name: string | null; owner_email: string | null; action: { type?: string; memory_id?: string; source_id?: string }; evidence: Record<string, unknown>; error: string | null; prompt: string | null };
type Search = { query: string | null; email: string; at: number };
const TABS = ["pending", "history", "searches"] as const;
const ACTIVE = ["pending", "queued", "waiting"];

export function Fixes() {
  const account = useAccount();
  const [tab, setTab] = useTabParam(TABS);
  const [, setParams] = useQueryParams();
  const [fixes, setFixes] = useState<Fix[]>([]);
  const [searches, setSearches] = useState<Search[]>([]);
  const [selected, setSelected] = useState<Fix | null>(null);
  const [detector, setDetector] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      const [rows, queries] = await Promise.all([cairn<Fix[]>("admin/fixes"), cairn<Search[]>("admin/searches")]);
      setFixes(rows); setSearches(queries); setError(null);
      window.dispatchEvent(new Event("cairn-fixes-changed"));
    } catch (e) { setError(e instanceof Error ? e.message : "Could not load fixes."); }
  }, []);
  useEffect(() => { if (account?.is_admin) void Promise.resolve().then(refresh); }, [account, refresh]);
  async function action(button: string) {
    if (!selected) return;
    setBusy(true);
    try {
      if (button === "Copy prompt") {
        if (!selected.prompt) throw new Error("No prompt is available.");
        await navigator.clipboard.writeText(selected.prompt);
      }
      const operation = button === "Apply" ? "apply" : button === "Ignore" ? "ignore" : button === "Mark done" ? "done" : "copied";
      const updated = await cairn<Fix>(`admin/fixes/${encodeURIComponent(selected.fix_id)}/${operation}`, { method: "POST", body: JSON.stringify(operation === "apply" ? { fingerprint: selected.fingerprint } : {}) });
      setSelected(updated); await refresh();
      toast.success(button === "Copy prompt" ? "Prompt copied" : "Fix updated");
    } catch (e) { toast.error(e instanceof Error ? e.message : "Could not update fix."); }
    finally { setBusy(false); }
  }
  const columns: ColumnDef<Fix>[] = [
    { accessorKey: "detector", header: "Detector", cell: ({ row }) => <Button variant="link" onClick={() => setSelected(row.original)}>{row.original.detector}</Button> },
    { accessorKey: "fix_type", header: "Type" }, { accessorKey: "status", header: "Status" },
    { id: "subject", header: "Subject", cell: ({ row }) => row.original.project_name ?? row.original.device_name ?? row.original.subject_id ?? "Server" },
  ];
  const table = (rows: Fix[]) => <DataTable columns={columns} data={rows} label="Fixes" filterPlaceholder="Filter fixes" empty={<p className="p-4 text-sm">No fixes here.</p>} />;
  if (account === undefined) return <p>Loading…</p>;
  if (!account?.is_admin) return <p>Admin access required.</p>;
  return <>
    {error ? <p role="alert">{error}</p> : null}
    <Tabs value={tab} onValueChange={(value) => setTab(value as typeof TABS[number])}>
      <TabsList><TabsTrigger value="pending">Pending</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="searches">Searches</TabsTrigger></TabsList>
      <TabsContent value="pending">{table(fixes.filter((fix) => ACTIVE.includes(fix.status)))}</TabsContent>
      <TabsContent value="history">
        <div className="flex gap-2 py-2"><label>Detector <select value={detector} onChange={(e) => setDetector(e.target.value)}><option value="">All</option>{Array.from(new Set(fixes.map((f) => f.detector))).sort().map((v) => <option key={v}>{v}</option>)}</select></label><label>Status <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">All</option>{Array.from(new Set(fixes.map((f) => f.status))).sort().map((v) => <option key={v}>{v}</option>)}</select></label></div>
        {table(fixes.filter((f) => !ACTIVE.includes(f.status) && (!detector || f.detector === detector) && (!status || f.status === status)))}
      </TabsContent>
      <TabsContent value="searches"><DataTable<Search> columns={[{accessorKey:"query",header:"Search phrase"},{accessorKey:"email",header:"Account"},{id:"at",header:"Time",cell:({row}) => new Date(row.original.at * 1000).toLocaleString()}]} data={searches} label="Searches" filterPlaceholder="Filter searches" empty={<p className="p-4">No recent searches.</p>} /></TabsContent>
    </Tabs>
    <Sheet open={selected !== null} onOpenChange={(open) => { if (!open) setSelected(null); }}><SheetContent className="overflow-y-auto"><SheetHeader><SheetTitle>{selected?.detector} fix</SheetTitle><SheetDescription>{selected?.status}</SheetDescription></SheetHeader>
      {selected ? <div className="space-y-4 px-4 pb-6 text-sm">
        <p>{selected.owner_email ? `Personal memory of ${selected.owner_email}` : selected.project_name ?? selected.device_name ?? selected.subject_id}</p>
        {selected.fix_type === "computer" ? <p>Device: {selected.device_name ?? selected.device_id}</p> : null}
        {selected.fix_type === "computer" ? <p>Runs on {selected.device_name ?? selected.device_id} the next time someone there runs cairn doctor, cairn init or cairn fixes apply.</p> : null}
        {selected.action.memory_id || selected.action.source_id ? <Button variant="outline" onClick={() => { const memory = selected.action.memory_id ?? selected.action.source_id; setSelected(null); setParams({memory}); }}>Open memory</Button> : null}
        {selected.owner_email && selected.prompt ? <p>Only {selected.owner_email} can run this prompt.</p> : null}
        {selected.status === "queued" ? <p>Queued for {selected.device_name}</p> : null}
        {selected.detector === "F6" && selected.fix_type === "computer" ? <p className="font-bold">This joins the person to the project.</p> : null}
        {selected.detector === "F9" ? <p className="font-bold">This removes the text for good. Rotate the secret now.</p> : null}
        <p>Change: {selected.action.type?.replaceAll("_", " ") ?? "Report"}</p>
        <pre className="whitespace-pre-wrap break-words">{JSON.stringify(selected.action, null, 2)}</pre>
        <pre className="whitespace-pre-wrap break-words">{JSON.stringify(selected.evidence, null, 2)}</pre>
        {selected.prompt ? <p className="whitespace-pre-wrap">{selected.prompt}</p> : null}
        {selected.error ? <p role="alert">{selected.error}</p> : null}
        <div className="flex flex-wrap gap-2">{buttonsFor(selected).map((button) => <Button key={button} disabled={busy} onClick={() => button === "Apply" && selected.detector === "F9" ? setConfirming(true) : void action(button)}>{button}</Button>)}</div>
      </div> : null}</SheetContent></Sheet>
    <AlertDialog open={confirming} onOpenChange={setConfirming}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Apply secret redaction?</AlertDialogTitle><AlertDialogDescription className="font-bold">This removes the text for good. Rotate the secret now.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction onClick={() => void action("Apply")}>Confirm</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </>;
}
