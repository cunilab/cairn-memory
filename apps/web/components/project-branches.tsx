"use client";

import { useState } from "react";
import { FormDialog } from "@/components/form-dialog";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SessionRequiredError } from "@/lib/session";
import { closeBranchImpact, mergeBranchImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import type { PersonalMemory, ProjectBranch } from "@/lib/types";

export function ProjectBranches({
  projectId,
  branches,
  submit,
  onSessionRequired,
}: {
  projectId: string;
  branches: ProjectBranch[];
  onSessionRequired: () => void;
  submit: (write: () => Promise<unknown>, success: string) => Promise<void>;
}) {
  const [action, setAction] = useState<{ branch: ProjectBranch; merge: boolean } | null>(null);
  const [memories, setMemories] = useState<PersonalMemory[]>([]);
  const [memoryId, setMemoryId] = useState("");
  const [commit, setCommit] = useState("");
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const path = `projects/${encodeURIComponent(projectId)}`;

  async function open(branch: ProjectBranch, merge: boolean) {
    setAction({ branch, merge });
    setReason("");
    setCommit("");
    setMemoryId("");
    setError(null);
    if (merge) {
      try {
        setMemories(
          (await cairn<PersonalMemory[]>(`${path}/memories`)).filter(
            (m) => m.context?.branch === branch.name,
          ),
        );
      } catch (cause) {
        if (cause instanceof SessionRequiredError) {
          onSessionRequired();
          return;
        }
        setError(cause instanceof Error ? cause.message : "Could not load branch memories");
      }
    }
  }

  return (
    <section className="mt-8 flex flex-col gap-4">
      <div>
        <h2 className="font-semibold">Your branches</h2>
        <p className="text-sm text-muted-foreground">
          These records belong to you. Recording a merge or closing a branch does not change another
          member’s branch status.
        </p>
      </div>
      {branches.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          Your branch records appear when you capture in this initialized repository.
        </p>
      ) : (
        branches.map((branch) => (
          <div
            key={branch.branch_id}
            className="flex flex-wrap items-center gap-3 rounded-md border p-3"
          >
            <span className="min-w-0 flex-1 text-sm break-all">
              {branch.name} <span className="text-muted-foreground">· {branch.status}</span>
            </span>
            {branch.status === "active" ? (
              <>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => void open(branch, true)}
                >
                  Record merge
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => void open(branch, false)}
                >
                  Close branch
                </Button>
              </>
            ) : null}
          </div>
        ))
      )}
      <FormDialog
        open={action !== null}
        onOpenChange={(open) => !open && setAction(null)}
        title={action?.merge ? "Record merge" : "Close branch"}
        description={
          action?.merge
            ? mergeBranchImpact(action.branch.name)
            : action
              ? closeBranchImpact(action.branch.name)
              : ""
        }
        submitLabel={action?.merge ? "Record merge" : "Close branch"}
        onSubmit={async () => {
          if (!action) return;
          await submit(
            () =>
              cairn(
                `${path}/branches/${encodeURIComponent(action.branch.branch_id)}/${action.merge ? "merge" : "close"}`,
                {
                  method: "POST",
                  body: JSON.stringify({
                    memoryId,
                    commit,
                    reason,
                  }),
                },
              ),
            action.merge ? "Merge recorded." : "Branch closed.",
          );
        }}
      >
        {action?.merge ? (
          <>
            <Field>
              <FieldLabel htmlFor="merge-memory">Your project evidence memory</FieldLabel>
              <Select
                value={memoryId || null}
                onValueChange={(value) => {
                  setMemoryId(value ?? "");
                              }}
              >
                <SelectTrigger id="merge-memory">
                  <SelectValue placeholder="Choose a memory" />
                </SelectTrigger>
                <SelectContent>
                  {memories.map((m) => (
                    <SelectItem key={m.memory_id} value={m.memory_id}>
                      {m.content.slice(0, 80)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            <Field>
              <FieldLabel htmlFor="merge-commit">Merge commit</FieldLabel>
              <Input
                id="merge-commit"
                value={commit}
                required
                onChange={(event) => {
                  setCommit(event.target.value);
                              }}
              />
            </Field>
          </>
        ) : null}
        <Field>
          <FieldLabel htmlFor="branch-reason">Reason</FieldLabel>
          <Input
            id="branch-reason"
            value={reason}
            required
            onChange={(event) => {
              setReason(event.target.value);
                      }}
          />
        </Field>

        {error ? (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}

      </FormDialog>
    </section>
  );
}
