"use client";

import { useEffect, useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { BrainIcon, MoreHorizontalIcon } from "lucide-react";
import { IdText, KindBadge } from "@/components/badges";
import { DataTable } from "@/components/data-table";
import { FormDialog } from "@/components/form-dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { PersonalMemory } from "@/lib/types";

/** The owner's personal memories for this project's repository, with Promote to decision. */
export function ProjectMemories({
  projectId,
  repository,
  submit,
  onSessionRequired,
}: {
  projectId: string;
  repository: string;
  /** Runs the write, reloads the project and toasts; throws so the dialog can show the error. */
  submit: (action: () => Promise<unknown>, success: string) => Promise<void>;
  onSessionRequired: () => void;
}) {
  const [memories, setMemories] = useState<PersonalMemory[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [promoting, setPromoting] = useState<PersonalMemory | null>(null);

  // shortcut: export returns every memory of the account, fine while personal stores are small.
  useEffect(() => {
    let cancelled = false;
    cairn<{ memories: PersonalMemory[]; forgottenIds?: string[] }>("export", { method: "POST" })
      .then((bundle) => {
        if (cancelled) {
          return;
        }
        const forgotten = new Set(bundle.forgottenIds ?? []);
        setMemories(
          bundle.memories.filter(
            (memory) =>
              memory.context?.repository === repository &&
              !memory.project_id &&
              memory.lifecycle?.sensitivity !== "secret" &&
              !forgotten.has(memory.memory_id),
          ),
        );
      })
      .catch((cause: unknown) => {
        if (cancelled) {
          return;
        }
        if (cause instanceof SessionRequiredError) {
          onSessionRequired();
          return;
        }
        setError(cause instanceof Error ? cause.message : "Could not load your memories");
      });
    return () => {
      cancelled = true;
    };
  }, [repository, onSessionRequired]);

  const columns = useMemo<ColumnDef<PersonalMemory>[]>(
    () => [
      {
        id: "memory",
        header: "Memory",
        accessorFn: (memory) => `${memory.memory_id} ${memory.content}`,
        enableSorting: false,
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-1.5">
            <p className="line-clamp-2 text-sm [overflow-wrap:anywhere]">{row.original.content}</p>
            <IdText id={row.original.memory_id} />
          </div>
        ),
      },
      {
        id: "kind",
        header: "Kind",
        accessorFn: (memory) => memory.kind ?? "",
        cell: ({ row }) => <KindBadge kind={row.original.kind} />,
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "branch",
        header: "Branch",
        accessorFn: (memory) => memory.context?.branch ?? "",
        cell: ({ row }) =>
          row.original.context?.branch ? (
            <span className="font-mono text-xs break-all">{row.original.context.branch}</span>
          ) : (
            <span className="text-muted-foreground">—</span>
          ),
        meta: { className: "hidden lg:table-cell" },
      },
      {
        id: "created",
        header: "Created",
        accessorFn: (memory) => Number(memory.created_at ?? 0),
        cell: ({ row }) => (
          <span className="text-xs whitespace-nowrap text-muted-foreground">
            {formatTime(row.original.created_at)}
          </span>
        ),
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        enableSorting: false,
        cell: ({ row }) => (
          <div className="flex justify-end">
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Actions for ${row.original.memory_id}`}
                  />
                }
              >
                <MoreHorizontalIcon />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-48">
                <DropdownMenuItem onClick={() => setPromoting(row.original)}>
                  Promote to decision
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ),
        meta: { className: "w-10" },
      },
    ],
    [],
  );

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <p className="max-w-prose text-sm text-muted-foreground">
        Everything Cairn records for you in this repository: captured prompts, tool results,
        session summaries and saved notes. Only you can see them. Promote one to turn it into a
        project decision.
      </p>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}
      <DataTable
        label="memories"
        data={memories ?? []}
        columns={columns}
        getRowId={(memory) => memory.memory_id}
        filterPlaceholder="Filter memories"
        initialSort={[{ id: "created", desc: true }]}
        empty={
          <Empty className="border-0 p-6">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <BrainIcon />
              </EmptyMedia>
              <EmptyTitle>{memories === null && !error ? "Loading memories…" : "No memories yet"}</EmptyTitle>
              <EmptyDescription>
                Memories appear here as your agent works in {repository}.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        }
      />
      <FormDialog
        open={promoting !== null}
        onOpenChange={(open) => !open && setPromoting(null)}
        title="Promote to decision"
        description={
          <>
            Copy this memory into the project as a decision. Your personal memory stays as it is.
            <span className="mt-2 line-clamp-3 block rounded-md bg-muted px-2.5 py-2 text-foreground [overflow-wrap:anywhere]">
              {promoting?.content}
            </span>
          </>
        }
        submitLabel="Promote"
        onSubmit={async (data) => {
          if (!promoting) {
            return;
          }
          const reason = String(data.get("reason")).trim();
          if (!reason) {
            throw new Error("Give a reason so the decision explains itself later.");
          }
          await submit(
            () =>
              cairn("promote", {
                method: "POST",
                body: JSON.stringify({
                  project_id: projectId,
                  memory_id: promoting.memory_id,
                  reason,
                }),
              }),
            "Memory promoted to a project decision.",
          );
        }}
      >
        <Field>
          <FieldLabel htmlFor="promote-reason">Reason</FieldLabel>
          <Input id="promote-reason" name="reason" autoComplete="off" required />
        </Field>
      </FormDialog>
    </div>
  );
}
