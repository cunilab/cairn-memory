import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { GraphView } from "./_components/graph-view";

export default function GraphPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Graph" }]}>
      <PageHeader title="Graph" description="How your memories connect. Select a memory to read it." />
      <Suspense fallback={<LoadingBlock label="Loading the graph…" rows={4} />}>
        <GraphView />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Graph" };
