import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { Skills } from "./_components/skills";

export default function SkillsPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Skills" }]}>
      <PageHeader title="Skills" description="Library, suggestions, and archived skills." />
      <Suspense fallback={<LoadingBlock label="Loading skills…" rows={4} />}>
        <Skills />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Skills" };
