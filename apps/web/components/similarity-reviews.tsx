"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { cairn } from "@/lib/cairn";
import type { ProjectMemory, SimilarityReview } from "@/lib/types";

export function SimilarityReviews({
  reviews,
  memories,
  onChanged,
}: {
  reviews: SimilarityReview[];
  memories: ProjectMemory[];
  onChanged: () => void;
}) {
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const title = (id: string) => memories.find((memory) => memory.memory_id === id)?.title ?? id;

  async function run(key: string, path: string, body: Record<string, string>) {
    setPending(key);
    setError(null);
    try {
      await cairn(path, { method: "POST", body: JSON.stringify(body) });
      onChanged();
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "Could not update the review");
    } finally {
      setPending(null);
    }
  }

  if (reviews.length === 0) {
    return null;
  }
  return (
    <section className="space-y-3" aria-label="Near-duplicate memories">
      <h2 className="text-sm font-medium">Near-duplicate memories</h2>
      <p className="text-sm text-muted-foreground">
        These saved notes look alike. Nothing changes until you dismiss, mark a contradiction, or supersede.
      </p>
      {reviews.map((review) => (
        <article key={review.review_id} className="space-y-2 rounded-md border p-3">
          <p className="text-sm">
            {title(review.left_id)} and {title(review.right_id)}
            <span className="text-muted-foreground">
              {" "}
              · {review.band === "near_duplicate" ? "near duplicate" : "possible conflict"} · cosine {review.cosine.toFixed(2)}
            </span>
          </p>
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="outline" disabled={pending !== null} onClick={() => run(`${review.review_id}-dismiss`, "similarity-reviews/dismiss", { reviewId: review.review_id })}>
              Dismiss
            </Button>
            <Button type="button" variant="outline" disabled={pending !== null} onClick={() => run(`${review.review_id}-left`, "similarity-reviews/contradict", { reviewId: review.review_id, contradictedId: review.left_id })}>
              Mark “{title(review.left_id)}” contradicted
            </Button>
            <Button type="button" variant="outline" disabled={pending !== null} onClick={() => run(`${review.review_id}-right`, "similarity-reviews/contradict", { reviewId: review.review_id, contradictedId: review.right_id })}>
              Mark “{title(review.right_id)}” contradicted
            </Button>
            <Button type="button" variant="outline" disabled={pending !== null} onClick={() => run(`${review.review_id}-super-left`, "similarity-reviews/supersede", { reviewId: review.review_id, successorId: review.left_id })}>
              “{title(review.left_id)}” supersedes
            </Button>
            <Button type="button" variant="outline" disabled={pending !== null} onClick={() => run(`${review.review_id}-super-right`, "similarity-reviews/supersede", { reviewId: review.review_id, successorId: review.right_id })}>
              “{title(review.right_id)}” supersedes
            </Button>
          </div>
        </article>
      ))}
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
    </section>
  );
}
