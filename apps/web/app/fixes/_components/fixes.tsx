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
import { cairn, cairnPage } from "@/lib/cairn";
import { buttonsFor, issueTitle } from "../_lib/fixes-buttons";

type Fix = Parameters<typeof buttonsFor>[0] & { fix_id: string; detector: string; fingerprint: string; subject_id: string | null; project_name: string | null; device_id: string | null; device_name: string | null; owner_email: string | null; action: { type?: string; memory_id?: string; source_id?: string }; evidence: Record<string, unknown>; error: string | null; prompt: string | null };
type Search = { query: string | null; email: string; at: number };
const TABS = ["action", "waiting", "history", "searches", "pending"] as const;
const NEEDS_ACTION = ["pending", "failed"];
const WAITING = ["queued", "waiting", "applying"];
const DETECTORS = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
const HISTORY_STATUS = ["applying", "done", "failed", "resolved", "ignored", "expired"];

function historyPath(detector: string, status: string, cursor: string | null) {
  const params = new URLSearchParams({ group: "history" });
  if (detector) params.set("detector", detector);
  if (status) params.set("status", status);
  if (cursor) params.set("cursor", cursor);
  return `admin/fixes?${params.toString()}`;
}

export function Fixes() {
  const account = useAccount();
  const [rawTab, setTab] = useTabParam(TABS);
  const tab = rawTab === "pending" ? "action" : rawTab;
  const [, setParams] = useQueryParams();
  const [fixes, setFixes] = useState<Fix[]>([]);
  const [history, setHistory] = useState<Fix[]>([]);
  const [historyNext, setHistoryNext] = useState<string | null>(null);
  const [searches, setSearches] = useState<Search[] | null>(null);
  const [selected, setSelected] = useState<Fix | null>(null);
  const [detector, setDetector] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(async () => {
    try {
      const rows = await cairn<Fix[]>("admin/fixes?group=open");
      setFixes(rows); setError(null); setRevision((n) => n + 1);
      window.dispatchEvent(new Event("cairn-fixes-changed"));
    } catch (e) { setError(e instanceof Error ? e.message : "Could not load fixes."); }
  }, []);
  useEffect(() => { if (account?.is_admin) void Promise.resolve().then(refresh); }, [account, refresh]);
  useEffect(() => {
    if (!account?.is_admin || tab !== "searches" || searches !== null) return;
    void cairn<Search[]>("admin/searches").then(setSearches).catch((e) => setError(e instanceof Error ? e.message : "Could not load searches."));
  }, [account, tab, searches]);
  useEffect(() => {
    if (!account?.is_admin || tab !== "history") return;
    let cancelled = false;
    void cairnPage<Fix>(historyPath(detector, status, null)).then((page) => {
      if (cancelled) return;
      setHistory(page.items); setHistoryNext(page.next);
    }).catch((e) => { if (!cancelled) setError(e instanceof Error ? e.message : "Could not load history."); });
    return () => { cancelled = true; };
  }, [account, tab, detector, status, revision]);
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
    { id: "issue", header: "Issue", cell: ({ row }) => <Button variant="link" className="h-auto whitespace-normal p-0 text-left" onClick={() => setSelected(row.original)}>{issueTitle(row.original.detector)}</Button> },
    { id: "subject", header: "Affected", cell: ({ row }) => row.original.project_name ?? row.original.device_name ?? row.original.subject_id ?? "Server" },
    { accessorKey: "status", header: "Status" },
  ];
  const table = (rows: Fix[], empty: string) => <DataTable columns={columns} data={rows} label="Fixes" filterPlaceholder="Filter fixes" empty={<p className="p-4 text-sm">{error ? "Could not load this list." : empty}</p>} />;
  if (account === undefined) return <p>Loading…</p>;
  if (!account?.is_admin) return <p className="text-sm text-muted-foreground">Only the admin can review and apply fixes.</p>;
  return <>
    {error ? <p role="alert">{error}</p> : null}
    <Tabs value={tab} onValueChange={(value) => setTab(value as typeof TABS[number])}>
      <TabsList><TabsTrigger value="action">Needs action</TabsTrigger><TabsTrigger value="waiting">Waiting</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="searches">Searches</TabsTrigger></TabsList>
      <TabsContent value="action">{table(fixes.filter((fix) => NEEDS_ACTION.includes(fix.status)), "No fixes need attention")}</TabsContent>
      <TabsContent value="waiting">{table(fixes.filter((fix) => WAITING.includes(fix.status)), "Nothing is waiting.")}</TabsContent>
      <TabsContent value="history">
        <div className="flex gap-2 py-2"><label>Detector <select value={detector} onChange={(e) => setDetector(e.target.value)}><option value="">All</option>{DETECTORS.map((v) => <option key={v}>{v}</option>)}</select></label><label>Status <select value={status} onChange={(e) => setStatus(e.target.value)}><option value="">All</option>{HISTORY_STATUS.map((v) => <option key={v}>{v}</option>)}</select></label></div>
        {table(history, "No history yet.")}
        {historyNext ? <Button variant="outline" className="mt-2" onClick={() => void cairnPage<Fix>(historyPath(detector, status, historyNext)).then((page) => { setHistory((rows) => [...rows, ...page.items]); setHistoryNext(page.next); }).catch((e) => setError(e instanceof Error ? e.message : "Could not load history."))}>More</Button> : null}
      </TabsContent>
      <TabsContent value="searches"><DataTable<Search> columns={[{accessorKey:"query",header:"Search phrase"},{accessorKey:"email",header:"Account"},{id:"at",header:"Time",cell:({row}) => new Date(row.original.at * 1000).toLocaleString()}]} data={searches ?? []} label="Searches" filterPlaceholder="Filter searches" empty={<p className="p-4">{searches === null ? "Loading…" : "No recent searches."}</p>} /></TabsContent>
    </Tabs>
    <Sheet open={selected !== null} onOpenChange={(open) => { if (!open) setSelected(null); }}><SheetContent className="w-full overflow-y-auto data-[side=right]:w-full data-[side=right]:sm:max-w-lg"><SheetHeader><SheetTitle>{selected ? issueTitle(selected.detector) : "Fix"}</SheetTitle><SheetDescription>{selected?.status}</SheetDescription></SheetHeader>
      {selected ? <div className="space-y-4 px-4 pb-6 text-sm">
        <p>{selected.owner_email ? `Personal memory of ${selected.owner_email}` : selected.project_name ?? selected.device_name ?? selected.subject_id}</p>
        {selected.fix_type === "computer" ? <p>Device: {selected.device_name ?? selected.device_id}</p> : null}
        {selected.fix_type === "computer" ? <p>Runs on {selected.device_name ?? selected.device_id} the next time someone there runs cairn doctor, cairn init or cairn fixes apply.</p> : null}
        {selected.action.memory_id || selected.action.source_id ? <Button variant="outline" onClick={() => { const memory = selected.action.memory_id ?? selected.action.source_id; setSelected(null); setParams({memory}); }}>Open memory</Button> : null}
        {selected.owner_email && selected.prompt ? <p>Only {selected.owner_email} can run this prompt.</p> : null}
        {selected.status === "queued" ? <p>Queued for {selected.device_name}</p> : null}
        {selected.detector === "F6" && selected.fix_type === "computer" ? <p className="font-bold">This joins the person to the project.</p> : null}
        {selected.detector === "F9" ? <p className="font-bold">This redacts the secret in the title, summary, reason, and kind fields. It can remain in evidence or an older version. Rotate the secret now.</p> : null}
        <p>Next: {selected.action.type?.replaceAll("_", " ") ?? "Report"}</p>
        <details>
          <summary className="cursor-pointer font-medium">Diagnostics</summary>
          <p className="mt-2 font-mono text-xs">{selected.detector}</p>
          <pre className="whitespace-pre-wrap break-words">{JSON.stringify(selected.action, null, 2)}</pre>
          <pre className="whitespace-pre-wrap break-words">{JSON.stringify(selected.evidence, null, 2)}</pre>
        </details>
        {selected.prompt ? <p className="whitespace-pre-wrap">{selected.prompt}</p> : null}
        {selected.error ? <p role="alert">{selected.error}</p> : null}
        <div className="flex flex-wrap gap-2">{buttonsFor(selected).map((button) => <Button key={button} disabled={busy} onClick={() => button === "Apply" && selected.detector === "F9" ? setConfirming(true) : void action(button)}>{button}</Button>)}</div>
      </div> : null}</SheetContent></Sheet>
    <AlertDialog open={confirming} onOpenChange={setConfirming}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Apply secret redaction?</AlertDialogTitle><AlertDialogDescription className="font-bold">This redacts the secret in the title, summary, reason, and kind fields. It can remain in evidence or an older version. Rotate the secret now.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction onClick={() => void action("Apply")}>Confirm</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </>;
}
