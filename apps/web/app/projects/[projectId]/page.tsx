import { Suspense } from "react";
import type { Metadata } from "next";
import { LoadingBlock } from "@/components/states";
import { ProjectDetail } from "../_components/project-detail";

async function ProjectBody({ params }: { params: Promise<{ projectId: string }> }) {
  const { projectId } = await params;
  return <ProjectDetail key={projectId} projectId={projectId} />;
}

export default function ProjectPage({ params }: { params: Promise<{ projectId: string }> }) {
  return (
    <Suspense
      fallback={
        <main className="mx-auto w-full max-w-3xl px-4 py-10">
          <LoadingBlock label="Loading project…" rows={4} />
        </main>
      }
    >
      <ProjectBody params={params} />
    </Suspense>
  );
}

export const metadata: Metadata = { title: "Project" };
