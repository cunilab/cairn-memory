import { Suspense } from "react";
import { ProjectDetail } from "@/components/project-detail";

async function ProjectBody({
  params,
}: {
  params: Promise<{ projectId: string }>;
}) {
  const { projectId } = await params;
  return <ProjectDetail key={projectId} projectId={projectId} />;
}

export default function ProjectPage({
  params,
}: {
  params: Promise<{ projectId: string }>;
}) {
  return (
    <Suspense
      fallback={
        <main className="mx-auto w-full max-w-3xl px-4 py-10 text-sm text-muted-foreground">
          Loading project…
        </main>
      }
    >
      <ProjectBody params={params} />
    </Suspense>
  );
}
