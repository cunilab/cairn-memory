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
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { PersonalMemory } from "@/lib/types";

/** The caller's personal repository memories, with preview before promotion. */
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
  const [reason, setReason] = useState("");
  const [preview, setPreview] = useState<{
    memoryId: string;
    reason: string;
    payload: Record<string, unknown>;
    fingerprint: string;
  } | null>(null);
  const previewCurrent =
    preview?.memoryId === promoting?.memory_id && preview?.reason === reason.trim();
  const [previewError, setPreviewError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    cairn<PersonalMemory[]>(`projects/${encodeURIComponent(projectId)}/personal-memories`)
      .then((rows) => {
        if (!cancelled) setMemories(rows);
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
  }, [projectId, onSessionRequired]);

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
                <DropdownMenuItem
                  onClick={() => {
                    setReason("");
                    setPreview(null);
                    setPreviewError(null);
                    setPromoting(row.original);
                  }}
                >
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
        Your live session summaries, lessons and saved notes for this repository. Only you can see
        them. Promote one to turn it into a project decision.
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
              <EmptyTitle>
                {memories === null && !error ? "Loading memories…" : "No memories yet"}
              </EmptyTitle>
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
            <span className="mt-2 block max-h-60 overflow-y-auto whitespace-pre-wrap rounded-md bg-muted px-2.5 py-2 text-foreground [overflow-wrap:anywhere]">
              {promoting?.content.slice(0, 200)}
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
          if (!previewCurrent)
            throw new Error("Preview the full payload before publishing it to every member.");
          await submit(
            () =>
              cairn("promote", {
                method: "POST",
                body: JSON.stringify({
                  project_id: projectId,
                  memory_id: promoting.memory_id,
                  reason,
                  previewFingerprint: preview?.fingerprint,
                }),
              }),
            "Memory promoted to a project decision.",
          );
        }}
      >
        <Field>
          <FieldLabel htmlFor="promote-reason">Reason</FieldLabel>
          <Input
            id="promote-reason"
            name="reason"
            autoComplete="off"
            value={reason}
            onChange={(event) => {
              setReason(event.target.value);
              setPreview(null);
            }}
            required
          />
        </Field>
        <Button
          type="button"
          variant="outline"
          disabled={!reason.trim()}
          onClick={async () => {
            setPreviewError(null);
            if (!promoting) return;
            const memoryId = promoting.memory_id;
            const previewReason = reason.trim();
            try {
              const response = await cairn<Record<string, unknown>>("promote", {
                method: "POST",
                body: JSON.stringify({ projectId, memoryId, reason: previewReason, dryRun: true }),
              });
              const {
                previewFingerprint,
                memory_id: _id,
                session_id: _session,
                created_at: _created,
                changed_at: _changed,
                ...payload
              } = response;
              void _id;
              void _session;
              void _created;
              void _changed;
              setPreview({
                memoryId,
                reason: previewReason,
                payload,
                fingerprint: String(previewFingerprint),
              });
            } catch (cause) {
              if (cause instanceof SessionRequiredError) {
                onSessionRequired();
                return;
              }
              setPreviewError(cause instanceof Error ? cause.message : "Preview failed");
            }
          }}
        >
          Preview full payload
        </Button>
        {previewError ? (
          <p role="alert" className="text-sm text-destructive">
            {previewError}
          </p>
        ) : null}
        {previewCurrent && preview ? (
          <div>
            <p className="text-sm">
              Every member will receive this payload, including the full content and metadata.
            </p>
            <pre className="max-h-80 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap break-all">
              {JSON.stringify(preview.payload, null, 2)}
            </pre>
          </div>
        ) : null}
      </FormDialog>
    </div>
  );
}
