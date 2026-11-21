import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { MemoryTable } from "./_components/memory-table";

export default function MemoriesPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Memories" }]}>
      <PageHeader
        title="Memories"
        description="What Cairn knows: project knowledge, your private preferences and lessons shared on this server."
      />
      <Suspense fallback={<LoadingBlock label="Loading memories…" rows={8} />}>
        <MemoryTable />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Memories" };
