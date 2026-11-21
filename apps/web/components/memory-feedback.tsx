"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { cairn } from "@/lib/cairn";
import type { ProjectMemory } from "@/lib/types";

export function MemoryFeedback({
  memory,
  onUpdated,
}: {
  memory: ProjectMemory;
  onUpdated: (memory: ProjectMemory) => void;
}) {
  const [pending, setPending] = useState<"helpful" | "wrong" | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function vote(vote: "helpful" | "wrong") {
    setPending(vote);
    setError(null);
    try {
      await cairn("feedback", {
        method: "POST",
        body: JSON.stringify({ id: memory.id, vote }),
      });
      onUpdated(await cairn<ProjectMemory>(`memories/${memory.id}`));
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "Could not save feedback");
    } finally {
      setPending(null);
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Button type="button" variant="outline" disabled={pending !== null} onClick={() => vote("helpful")}>
        Helpful
      </Button>
      <Button type="button" variant="outline" disabled={pending !== null} onClick={() => vote("wrong")}>
        Wrong
      </Button>
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
    </div>
  );
}
