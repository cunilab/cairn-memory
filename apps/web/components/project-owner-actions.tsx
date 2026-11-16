"use client";

import { useEffect, useState, type ReactNode } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { closeBranchImpact, mergeBranchImpact, sharePolicyImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";
import type { PersonalMemory, ProjectBranch, ProjectMemory } from "@/lib/types";

const POLICIES = [
  { value: "manual", label: "Manual" },
  { value: "auto_nonsensitive_project_memory", label: "Automatic" },
];

type Confirm = { title: string; description: string; label: string; go: () => void };

function pick<T>(items: T[], key: (item: T) => string, id: string): T | undefined {
  return items.find((item) => key(item) === id) ?? items[0];
}

function preview(memory: PersonalMemory): string {
  const text = memory.content.replace(/\s+/g, " ");
  return `${memory.memory_id} · ${text.length > 60 ? `${text.slice(0, 60)}…` : text}`;
}

type Option = { value: string; label: string };

/** A labelled select that shows `empty` and stays disabled when there is nothing to choose. */
function Choice({
  id,
  value,
  options,
  empty,
  onChange,
}: {
  id: string;
  value: string;
  options: Option[];
  empty: string;
  onChange: (value: string) => void;
}) {
  return (
    <Select
      items={options}
      value={options.length === 0 ? null : value}
      onValueChange={(next) => next && onChange(next)}
      disabled={options.length === 0}
    >
      <SelectTrigger id={id} className="w-full min-w-0">
        <SelectValue placeholder={empty} />
      </SelectTrigger>
      <SelectContent className="max-w-[min(34rem,calc(100vw-2rem))]">
        {options.map((option) => (
          <SelectItem key={option.value} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function ActionCard({
  title,
  description,
  footer,
  children,
}: {
  title: string;
  description: string;
  footer?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <Card className="h-full min-w-0">
      <CardHeader>
        <CardTitle>
          <h3>{title}</h3>
        </CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      {children ? <CardContent className="flex min-w-0 flex-col gap-4">{children}</CardContent> : null}
      {footer ? <CardFooter className="mt-auto">{footer}</CardFooter> : null}
    </Card>
  );
}

function BranchStatus({ status }: { status: string }) {
  return (
    <Badge variant={status === "merged" ? "success" : status === "closed" ? "secondary" : "outline"}>
      {status.charAt(0).toUpperCase() + status.slice(1)}
    </Badge>
  );
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
  const branchOptions = openBranches.map((branch) => ({ value: branch.branch_id, label: branch.name }));

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {memoriesError ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{memoriesError}</AlertDescription>
        </Alert>
      ) : null}

      <div className="grid min-w-0 gap-4 lg:grid-cols-2">
        <ActionCard
          title="Share policy"
          description="Decides whether new decisions reach members on their own."
        >
          <p className="flex items-center gap-2 text-sm">
            Current
            <Badge variant="brand">
              {POLICIES.find((policy) => policy.value === sharePolicy)?.label ??
                "not reported by the server"}
            </Badge>
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
        </ActionCard>

        <ActionCard
          title="Branches"
          description="Branches registered for this project and where each one stands."
        >
          {branches.length === 0 ? (
            <p className="text-sm text-muted-foreground">No branches registered.</p>
          ) : (
            <ul className="divide-y rounded-lg border text-sm">
              {branches.map((branch) => (
                <li key={branch.branch_id} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
                  <span className="min-w-0 flex-1 font-mono text-xs break-all">{branch.name}</span>
                  {branch.merge_commit ? (
                    <span className="font-mono text-xs break-all text-muted-foreground">
                      merged at {branch.merge_commit}
                    </span>
                  ) : null}
                  <BranchStatus status={branch.status} />
                </li>
              ))}
            </ul>
          )}
        </ActionCard>

        <form
          className="min-w-0"
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
          <ActionCard
            title="Promote a memory"
            description="Copy one of your personal memories into this project as a decision."
            footer={
              <Button type="submit" disabled={pending || !promote || promoteReason.trim() === ""}>
                Promote memory
              </Button>
            }
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="promote-memory">Personal memory</FieldLabel>
                <Choice
                  id="promote-memory"
                  value={promote?.memory_id ?? ""}
                  options={memories.map((memory) => ({
                    value: memory.memory_id,
                    label: preview(memory),
                  }))}
                  empty="No personal memories"
                  onChange={setPromoteId}
                />
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
          </ActionCard>
        </form>

        <form
          className="min-w-0"
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
          <ActionCard
            title="Verify a decision"
            description="Tie a decision to the commit where you checked that it still holds."
            footer={
              <Button type="submit" disabled={pending || !verify || verifyCommit.trim() === ""}>
                Verify decision
              </Button>
            }
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="verify-memory">Decision</FieldLabel>
                <Choice
                  id="verify-memory"
                  value={verify?.memory_id ?? ""}
                  options={decisions.map((memory) => ({
                    value: memory.memory_id,
                    label: memory.memory_id,
                  }))}
                  empty="No decisions"
                  onChange={setVerifyId}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="verify-commit">Commit</FieldLabel>
                <Input
                  id="verify-commit"
                  className="font-mono"
                  value={verifyCommit}
                  onChange={(event) => setVerifyCommit(event.target.value)}
                  autoComplete="off"
                  required
                />
              </Field>
            </FieldGroup>
          </ActionCard>
        </form>

        <form
          className="min-w-0"
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
          <ActionCard
            title="Merge a branch"
            description="Turn one branch memory into a project decision and mark the branch merged."
            footer={
              <Button
                type="submit"
                disabled={
                  pending ||
                  !mergeBranch ||
                  !mergeMemory ||
                  mergeCommit.trim() === "" ||
                  mergeReason.trim() === ""
                }
              >
                Merge branch
              </Button>
            }
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="merge-branch">Branch</FieldLabel>
                <Choice
                  id="merge-branch"
                  value={mergeBranch?.branch_id ?? ""}
                  options={branchOptions}
                  empty="No open branches"
                  onChange={setMergeBranchId}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="merge-memory">Branch memory to merge</FieldLabel>
                <Choice
                  id="merge-memory"
                  value={mergeMemory?.memory_id ?? ""}
                  options={mergeMemories.map((memory) => ({
                    value: memory.memory_id,
                    label: preview(memory),
                  }))}
                  empty="No memories captured on this branch"
                  onChange={setMergeMemoryId}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="merge-commit">Merge commit</FieldLabel>
                <Input
                  id="merge-commit"
                  className="font-mono"
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
          </ActionCard>
        </form>

        <form
          className="min-w-0"
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
          <ActionCard
            title="Close a branch"
            description="Retire a branch without merging it. A closed branch cannot be merged later."
            footer={
              <Button
                type="submit"
                variant="destructive"
                disabled={pending || !closeBranch || closeReason.trim() === ""}
              >
                Close branch
              </Button>
            }
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="close-branch">Branch</FieldLabel>
                <Choice
                  id="close-branch"
                  value={closeBranch?.branch_id ?? ""}
                  options={branchOptions}
                  empty="No open branches"
                  onChange={setCloseBranchId}
                />
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
          </ActionCard>
        </form>
      </div>

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
    </div>
  );
}
