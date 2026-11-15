"use client";

import { useEffect, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { closeBranchImpact, mergeBranchImpact, sharePolicyImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";
import type { PersonalMemory, ProjectBranch, ProjectMemory } from "@/lib/types";

const POLICIES = [
  { value: "manual", label: "Manual" },
  { value: "auto_nonsensitive_project_memory", label: "Automatic" },
];

const SELECT_CLASS =
  "h-8 w-full min-w-0 rounded-lg border border-input bg-transparent px-2.5 py-1 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50 md:text-sm dark:bg-input/30 dark:*:bg-popover";

type Confirm = { title: string; description: string; label: string; go: () => void };

function pick<T>(items: T[], key: (item: T) => string, id: string): T | undefined {
  return items.find((item) => key(item) === id) ?? items[0];
}

function preview(memory: PersonalMemory): string {
  const text = memory.content.replace(/\s+/g, " ");
  return `${memory.memory_id} · ${text.length > 60 ? `${text.slice(0, 60)}…` : text}`;
}

/** Owner-only writes on a project. `run` reloads the project and reports errors above the form. */
export function ProjectOwnerActions({
  projectId,
  sharePolicy,
  branches,
  decisions,
  pending,
  run,
  onSessionRequired,
}: {
  projectId: string;
  sharePolicy: string | null | undefined;
  branches: ProjectBranch[];
  decisions: ProjectMemory[];
  pending: boolean;
  run: (action: () => Promise<unknown>, success?: string) => Promise<boolean>;
  onSessionRequired: () => void;
}) {
  const base = `projects/${encodeURIComponent(projectId)}`;
  const [memories, setMemories] = useState<PersonalMemory[]>([]);
  const [memoriesError, setMemoriesError] = useState<string | null>(null);
  const [promoteId, setPromoteId] = useState("");
  const [promoteReason, setPromoteReason] = useState("");
  const [verifyId, setVerifyId] = useState("");
  const [verifyCommit, setVerifyCommit] = useState("");
  const [mergeBranchId, setMergeBranchId] = useState("");
  const [mergeMemoryId, setMergeMemoryId] = useState("");
  const [mergeCommit, setMergeCommit] = useState("");
  const [mergeReason, setMergeReason] = useState("");
  const [closeBranchId, setCloseBranchId] = useState("");
  const [closeReason, setCloseReason] = useState("");
  const [confirm, setConfirm] = useState<Confirm | null>(null);

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
              memory.scope !== "project" &&
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
        setMemoriesError(cause instanceof Error ? cause.message : "Could not load your memories");
      });
    return () => {
      cancelled = true;
    };
  }, [onSessionRequired]);

  const openBranches = branches.filter(
    (branch) => branch.status !== "merged" && branch.status !== "closed",
  );
  const promote = pick(memories, (m) => m.memory_id, promoteId);
  const verify = pick(decisions, (m) => m.memory_id, verifyId);
  const mergeBranch = pick(openBranches, (b) => b.branch_id, mergeBranchId);
  const mergeMemories = memories.filter(
    (memory) => memory.context?.branch === mergeBranch?.name,
  );
  const mergeMemory = pick(mergeMemories, (m) => m.memory_id, mergeMemoryId);
  const closeBranch = pick(openBranches, (b) => b.branch_id, closeBranchId);

  return (
    <section className="flex min-w-0 flex-col gap-6">
      <h2 className="text-base font-medium">Project controls</h2>
      {memoriesError ? (
        <p role="alert" className="text-sm text-destructive">
          {memoriesError}
        </p>
      ) : null}

      <div className="flex max-w-xl min-w-0 flex-col gap-3">
        <h3 className="text-sm font-medium">Share policy</h3>
        <p className="text-sm text-muted-foreground">
          Current:{" "}
          {POLICIES.find((policy) => policy.value === sharePolicy)?.label ??
            "not reported by the server"}
        </p>
        <div className="flex flex-wrap gap-2">
          {POLICIES.filter((policy) => policy.value !== sharePolicy).map((policy) => (
            <Button
              key={policy.value}
              type="button"
              variant="outline"
              disabled={pending}
              onClick={() =>
                setConfirm({
                  title: `Switch to ${policy.label.toLowerCase()} sharing?`,
                  description: sharePolicyImpact(policy.value),
                  label: `Switch to ${policy.label.toLowerCase()}`,
                  go: () =>
                    void run(
                      () =>
                        cairn(`${base}/policy`, {
                          method: "POST",
                          body: JSON.stringify({ policy: policy.value }),
                        }),
                      `Share policy set to ${policy.label.toLowerCase()}.`,
                    ),
                })
              }
            >
              Use {policy.label.toLowerCase()} sharing
            </Button>
          ))}
        </div>
      </div>

      <form
        className="flex max-w-xl min-w-0 flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (!promote) {
            return;
          }
          void run(
            () =>
              cairn("promote", {
                method: "POST",
                body: JSON.stringify({
                  project_id: projectId,
                  memory_id: promote.memory_id,
                  reason: promoteReason,
                }),
              }),
            "Memory promoted to a project decision.",
          ).then((ok) => ok && setPromoteReason(""));
        }}
      >
        <h3 className="text-sm font-medium">Promote a memory</h3>
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="promote-memory">Personal memory</FieldLabel>
            <select
              id="promote-memory"
              className={SELECT_CLASS}
              value={promote?.memory_id ?? ""}
              onChange={(event) => setPromoteId(event.target.value)}
              disabled={memories.length === 0}
            >
              {memories.length === 0 ? <option value="">No personal memories</option> : null}
              {memories.map((memory) => (
                <option key={memory.memory_id} value={memory.memory_id}>
                  {preview(memory)}
                </option>
              ))}
            </select>
          </Field>
          <Field>
            <FieldLabel htmlFor="promote-reason">Reason</FieldLabel>
            <Input
              id="promote-reason"
              value={promoteReason}
              onChange={(event) => setPromoteReason(event.target.value)}
              autoComplete="off"
              required
            />
          </Field>
        </FieldGroup>
        <div>
          <Button type="submit" disabled={pending || !promote || promoteReason.trim() === ""}>
            Promote memory
          </Button>
        </div>
      </form>

      <form
        className="flex max-w-xl min-w-0 flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (!verify) {
            return;
          }
          void run(
            () =>
              cairn("projects/verify", {
                method: "POST",
                body: JSON.stringify({
                  project_id: projectId,
                  memory_id: verify.memory_id,
                  commit: verifyCommit.trim(),
                }),
              }),
            "Decision verified.",
          ).then((ok) => ok && setVerifyCommit(""));
        }}
      >
        <h3 className="text-sm font-medium">Verify a decision</h3>
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="verify-memory">Decision</FieldLabel>
            <select
              id="verify-memory"
              className={SELECT_CLASS}
              value={verify?.memory_id ?? ""}
              onChange={(event) => setVerifyId(event.target.value)}
              disabled={decisions.length === 0}
            >
              {decisions.length === 0 ? <option value="">No decisions</option> : null}
              {decisions.map((memory) => (
                <option key={memory.memory_id} value={memory.memory_id}>
                  {memory.memory_id}
                </option>
              ))}
            </select>
          </Field>
          <Field>
            <FieldLabel htmlFor="verify-commit">Commit</FieldLabel>
            <Input
              id="verify-commit"
              value={verifyCommit}
              onChange={(event) => setVerifyCommit(event.target.value)}
              autoComplete="off"
              required
            />
          </Field>
        </FieldGroup>
        <div>
          <Button type="submit" disabled={pending || !verify || verifyCommit.trim() === ""}>
            Verify decision
          </Button>
        </div>
      </form>

      <div className="flex max-w-xl min-w-0 flex-col gap-3">
        <h3 className="text-sm font-medium">Branches</h3>
        {branches.length === 0 ? (
          <p className="text-sm text-muted-foreground">No branches registered.</p>
        ) : (
          <ul className="text-sm">
            {branches.map((branch) => (
              <li key={branch.branch_id} className="break-all">
                {branch.name} · {branch.status}
                {branch.merge_commit ? ` · merged at ${branch.merge_commit}` : ""}
              </li>
            ))}
          </ul>
        )}
      </div>

      <form
        className="flex max-w-xl min-w-0 flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (!mergeBranch || !mergeMemory) {
            return;
          }
          const [branch, memory] = [mergeBranch, mergeMemory];
          setConfirm({
            title: "Merge this branch?",
            description: mergeBranchImpact(branch.name),
            label: "Merge branch",
            go: () =>
              void run(
                () =>
                  cairn(`${base}/branches/${encodeURIComponent(branch.branch_id)}/merge`, {
                    method: "POST",
                    body: JSON.stringify({
                      memory_id: memory.memory_id,
                      commit: mergeCommit.trim(),
                      reason: mergeReason.trim(),
                    }),
                  }),
                `Branch ${branch.name} merged.`,
              ).then((ok) => {
                if (ok) {
                  setMergeCommit("");
                  setMergeReason("");
                }
              }),
          });
        }}
      >
        <h3 className="text-sm font-medium">Merge a branch</h3>
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="merge-branch">Branch</FieldLabel>
            <select
              id="merge-branch"
              className={SELECT_CLASS}
              value={mergeBranch?.branch_id ?? ""}
              onChange={(event) => setMergeBranchId(event.target.value)}
              disabled={openBranches.length === 0}
            >
              {openBranches.length === 0 ? <option value="">No open branches</option> : null}
              {openBranches.map((branch) => (
                <option key={branch.branch_id} value={branch.branch_id}>
                  {branch.name}
                </option>
              ))}
            </select>
          </Field>
          <Field>
            <FieldLabel htmlFor="merge-memory">Branch memory to merge</FieldLabel>
            <select
              id="merge-memory"
              className={SELECT_CLASS}
              value={mergeMemory?.memory_id ?? ""}
              onChange={(event) => setMergeMemoryId(event.target.value)}
              disabled={mergeMemories.length === 0}
            >
              {mergeMemories.length === 0 ? (
                <option value="">No memories captured on this branch</option>
              ) : null}
              {mergeMemories.map((memory) => (
                <option key={memory.memory_id} value={memory.memory_id}>
                  {preview(memory)}
                </option>
              ))}
            </select>
          </Field>
          <Field>
            <FieldLabel htmlFor="merge-commit">Merge commit</FieldLabel>
            <Input
              id="merge-commit"
              value={mergeCommit}
              onChange={(event) => setMergeCommit(event.target.value)}
              autoComplete="off"
              required
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="merge-reason">Reason</FieldLabel>
            <Input
              id="merge-reason"
              value={mergeReason}
              onChange={(event) => setMergeReason(event.target.value)}
              autoComplete="off"
              required
            />
          </Field>
        </FieldGroup>
        <div>
          <Button
            type="submit"
            disabled={
              pending || !mergeBranch || !mergeMemory || mergeCommit.trim() === "" || mergeReason.trim() === ""
            }
          >
            Merge branch
          </Button>
        </div>
      </form>

      <form
        className="flex max-w-xl min-w-0 flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (!closeBranch) {
            return;
          }
          const branch = closeBranch;
          setConfirm({
            title: "Close this branch?",
            description: closeBranchImpact(branch.name),
            label: "Close branch",
            go: () =>
              void run(
                () =>
                  cairn(`${base}/branches/${encodeURIComponent(branch.branch_id)}/close`, {
                    method: "POST",
                    body: JSON.stringify({ reason: closeReason.trim() }),
                  }),
                `Branch ${branch.name} closed.`,
              ).then((ok) => ok && setCloseReason("")),
          });
        }}
      >
        <h3 className="text-sm font-medium">Close a branch</h3>
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="close-branch">Branch</FieldLabel>
            <select
              id="close-branch"
              className={SELECT_CLASS}
              value={closeBranch?.branch_id ?? ""}
              onChange={(event) => setCloseBranchId(event.target.value)}
              disabled={openBranches.length === 0}
            >
              {openBranches.length === 0 ? <option value="">No open branches</option> : null}
              {openBranches.map((branch) => (
                <option key={branch.branch_id} value={branch.branch_id}>
                  {branch.name}
                </option>
              ))}
            </select>
          </Field>
          <Field>
            <FieldLabel htmlFor="close-reason">Reason</FieldLabel>
            <Input
              id="close-reason"
              value={closeReason}
              onChange={(event) => setCloseReason(event.target.value)}
              autoComplete="off"
              required
            />
          </Field>
        </FieldGroup>
        <div>
          <Button
            type="submit"
            variant="destructive"
            disabled={pending || !closeBranch || closeReason.trim() === ""}
          >
            Close branch
          </Button>
        </div>
      </form>

      <AlertDialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open) {
            setConfirm(null);
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{confirm?.title}</AlertDialogTitle>
            <AlertDialogDescription className="text-left [overflow-wrap:anywhere]">
              {confirm?.description}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                confirm?.go();
                setConfirm(null);
              }}
            >
              {confirm?.label}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
