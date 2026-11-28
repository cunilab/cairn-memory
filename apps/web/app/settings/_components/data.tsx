"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { DownloadIcon, UploadIcon } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cairn } from "@/lib/cairn";
import { downloadExport } from "@/lib/export";
import type { ImportReport } from "@/lib/generated/ImportReport";
import { announceMemoriesChanged } from "@/lib/memory-events";
import { loginHref, SessionRequiredError } from "@/lib/session";

type Dry = { body: string; report: ImportReport };
type TraceRow = { id: string; query: string; created_at: number; results: { id: string; rank: number; channel: string; opened: boolean; voted: boolean }[] };
type TraceReply = { enabled: boolean; traces: TraceRow[] };

/** Export what you own; import shows a dry-run summary first and writes only on confirm. */
export function DataTab() {
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [dry, setDry] = useState<Dry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [trace, setTrace] = useState<TraceReply | null>(null);

  useEffect(() => {
    let live = true;
    cairn<TraceReply>("me/trace")
      .then((reply) => {
        if (live) setTrace(reply);
      })
      .catch((cause) => {
        if (!live) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(loginHref());
          return;
        }
        setError(cause instanceof Error ? cause.message : "Could not load the search trace.");
      });
    return () => {
      live = false;
    };
  }, [router]);

  function fail(cause: unknown, fallback: string) {
    if (cause instanceof SessionRequiredError) {
      router.replace(loginHref());
      return;
    }
    setError(cause instanceof Error ? cause.message : fallback);
  }

  async function exportAll() {
    setBusy(true);
    setError(null);
    try {
      await downloadExport();
      toast.success("Export ready");
    } catch (cause) {
      fail(cause, "Could not export. Check that the server is running, then try again.");
    } finally {
      setBusy(false);
    }
  }

  async function choose(file: File | undefined) {
    setDry(null);
    setError(null);
    if (!file) return;
    setBusy(true);
    try {
      const body = await file.text();
      try {
        JSON.parse(body);
      } catch {
        setError("That file is not valid JSON. Choose a file made by Export.");
        return;
      }
      setDry({ body, report: await cairn<ImportReport>("memories/import?dry_run=1", { method: "POST", body }) });
    } catch (cause) {
      fail(cause, "Could not read this file.");
    } finally {
      setBusy(false);
    }
  }

  async function importNow() {
    if (!dry) return;
    setBusy(true);
    setError(null);
    try {
      const report = await cairn<ImportReport>("memories/import", { method: "POST", body: dry.body });
      toast.success(
        `Imported ${report.new.toLocaleString()} ${report.new === 1 ? "memory" : "memories"}${report.invalid.length > 0 ? `, ${report.invalid.length.toLocaleString()} skipped` : ""}`,
      );
      announceMemoriesChanged();
      setDry(null);
    } catch (cause) {
      fail(cause, "Could not import.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex max-w-xl flex-col gap-6">
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Search trace</h2>
        <p className="text-sm text-muted-foreground">
          When this is on, Cairn keeps your searches for 7 days: a short redacted query, result ids, ranks and
          channels. Memory text is not stored. Only you can read this list.
        </p>
        <Label className="flex min-h-11 items-center gap-2 text-sm">
          <input
            type="checkbox"
            className="size-4"
            checked={trace?.enabled === true}
            disabled={busy || trace === null}
            onChange={(event) => {
              const enabled = event.target.checked;
              setBusy(true);
              setError(null);
              void cairn<TraceReply>("me/trace", { method: "POST", body: JSON.stringify({ enabled }) })
                .then(setTrace)
                .catch((cause) => fail(cause, "Could not update the search trace."))
                .finally(() => setBusy(false));
            }}
          />
          Keep a search trace
        </Label>
        {trace && trace.traces.length > 0 ? (
          <ul className="flex flex-col gap-2 text-sm">
            {trace.traces.map((row) => (
              <li key={row.id} className="rounded-md border px-3 py-2">
                <p>{row.query}</p>
                <p className="text-muted-foreground">
                  {row.results.map((item) => `${item.rank}. ${item.id} (${item.channel})`).join(", ") || "No results"}
                </p>
              </li>
            ))}
          </ul>
        ) : null}
      </section>
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Export</h2>
        <p className="text-sm text-muted-foreground">Download all memories you own as JSON.</p>
        <div>
          <Button type="button" variant="outline" disabled={busy} onClick={() => void exportAll()}>
            <DownloadIcon />
            Export
          </Button>
        </div>
      </section>
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Import</h2>
        <p className="text-sm text-muted-foreground">
          Choose a Cairn export file. You see a summary of what would be added before anything is written. A project
          memory is added only for a project you already belong to.
        </p>
        <Label htmlFor="import-file" className="sr-only">Cairn export file</Label>
        <Input id="import-file" type="file" accept=".json,application/json" disabled={busy} onChange={(event) => void choose(event.target.files?.[0])} />
        {dry ? (
          <div className="flex flex-col gap-2" aria-live="polite">
            <p className="text-sm tabular-nums">
              {dry.report.new.toLocaleString()} new, {dry.report.duplicate.toLocaleString()} already present,{" "}
              {dry.report.invalid.length.toLocaleString()} invalid
            </p>
            {dry.report.invalid.length > 0 ? (
              <ul className="max-h-40 overflow-y-auto text-sm text-muted-foreground">
                {dry.report.invalid.map((item) => (
                  <li key={item.index}>
                    Memory {item.index + 1}: {item.error}
                  </li>
                ))}
              </ul>
            ) : null}
            <div>
              <Button type="button" disabled={busy || dry.report.new === 0} onClick={() => void importNow()}>
                <UploadIcon />
                Import {dry.report.new.toLocaleString()} {dry.report.new === 1 ? "memory" : "memories"}
              </Button>
            </div>
          </div>
        ) : null}
      </section>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}
