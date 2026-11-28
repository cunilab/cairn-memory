"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useOpenMemory, useQueryParams } from "@/hooks/use-query-params";
import { cairn } from "@/lib/cairn";
import type { SessionInspection } from "@/lib/generated/SessionInspection";
import type { SessionOutput } from "@/lib/generated/SessionOutput";
import { sessionTitle } from "@/lib/provenance";

export function SessionInspector() {
  const [params] = useQueryParams();
  const openMemory = useOpenMemory();
  const person = params.get("person") ?? "";
  const session = params.get("session") ?? "";
  const [report, setReport] = useState<SessionInspection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!person || !session) return;
    let active = true;
    const query = new URLSearchParams({ person_id: person, session_id: session });
    cairn<SessionInspection>(`memories/session?${query}`)
      .then((body) => {
        if (active) {
          setReport(body);
          setError(null);
        }
      })
      .catch((cause: Error) => {
        if (active) setError(cause.message);
      });
    return () => {
      active = false;
    };
  }, [person, session, attempt]);

  if (!person || !session) {
    return <p className="text-sm text-muted-foreground">This page needs a session link.</p>;
  }
  if (error) return <ErrorAlert message={error} onRetry={() => { setError(null); setAttempt((n) => n + 1); }} />;
  if (!report) return <LoadingBlock label="Loading session…" rows={6} />;

  const title = sessionTitle(report.session.branch);
  return (
    <div className="flex flex-col gap-4">
      <h1 className="text-2xl font-semibold">{title}</h1>
      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="outputs">Outputs {report.outputs.length}</TabsTrigger>
          <TabsTrigger value="fixes">Related fixes</TabsTrigger>
        </TabsList>
        <TabsContent value="overview" className="space-y-2 text-sm">
          <p>Started <Time value={report.session.started_at} /></p>
          <p>Ended <Time value={report.session.ended_at} /></p>
          <p>Branch {report.session.branch ?? "—"}</p>
          <p>Notes {report.session.notes_written ?? "—"}</p>
          <p>Digest {report.session.digest_written == null ? "—" : report.session.digest_written ? "written" : "not written"}</p>
          <p>Handoff {report.session.handoff_outcome ?? "—"}</p>
          <p>Files {report.session.files_changed ?? "—"}</p>
          <p>New records in this time window {report.session.agent_saves}</p>
          {report.project_id ? (
            <Button nativeButton={false} variant="outline" size="sm" render={<Link href={`/projects/${encodeURIComponent(report.project_id)}?tab=sessions`} />}>
              Project sessions
            </Button>
          ) : null}
        </TabsContent>
        <TabsContent value="outputs">
          <OutputList rows={report.outputs} onOpen={openMemory} empty="This session has no visible memories." />
        </TabsContent>
        <TabsContent value="fixes">
          <OutputList rows={report.fixes} onOpen={openMemory} empty="No related fixes are visible." />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function OutputList({ rows, onOpen, empty }: { rows: SessionOutput[]; onOpen: (id: string) => void; empty: string }) {
  if (rows.length === 0) return <p className="text-sm text-muted-foreground">{empty}</p>;
  return (
    <ul className="divide-y rounded-lg border">
      {rows.map((row) => (
        <li key={row.id} className="flex flex-wrap items-baseline justify-between gap-2 px-3 py-2">
          <button type="button" className="text-left text-sm font-medium underline-offset-4 hover:underline" onClick={() => onOpen(row.id)}>
            {row.title}
          </button>
          <span className="text-xs text-muted-foreground">{row.kind} · {row.role}</span>
        </li>
      ))}
    </ul>
  );
}
