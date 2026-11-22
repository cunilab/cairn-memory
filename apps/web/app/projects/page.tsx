import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { ProjectList } from "./_components/project-list";

export default function ProjectsPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Projects" }]}>
      <PageHeader
        title="Projects"
        description="Repositories you work on with Cairn. Every member can read the project's memories."
      />
      <Suspense fallback={<LoadingBlock label="Loading projects…" rows={4} />}>
        <ProjectList />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Projects" };
