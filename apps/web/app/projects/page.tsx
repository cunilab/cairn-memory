"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { cairn } from "@/lib/cairn";
import type { ListedProject } from "@/lib/types";

export default function ProjectsPage() {
  const router = useRouter();
  const [projects, setProjects] = useState<ListedProject[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    cairn<ListedProject[]>("projects")
      .then((listed) => {
        if (!cancelled) {
          setProjects(listed);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : "Could not load projects");
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function logout() {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Logout failed");
      return;
    }
    router.push("/");
  }

  return (
    <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-10">
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-xl font-medium">Projects</h1>
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      </div>
      {error ? (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      {loading ? <p className="text-sm text-muted-foreground">Loading projects…</p> : null}
      {!loading && projects.length === 0 && !error ? (
        <p className="text-sm text-muted-foreground">No projects for this account.</p>
      ) : null}
      <ul className="flex flex-col gap-3">
        {projects.map((project) => (
          <li key={project.project_id}>
            <Card>
              <CardHeader>
                <CardTitle>
                  <Link href={`/projects/${project.project_id}`}>{project.name}</Link>
                </CardTitle>
                <CardDescription>
                  {project.repository} · {project.role}
                </CardDescription>
              </CardHeader>
              <CardContent className="text-sm text-muted-foreground">
                {project.project_id}
              </CardContent>
            </Card>
          </li>
        ))}
      </ul>
    </main>
  );
}
