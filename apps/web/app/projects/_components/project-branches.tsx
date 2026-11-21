"use client";

import { Fragment, useState } from "react";
import { ChevronRightIcon } from "lucide-react";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { cn } from "@/lib/utils";
import type { ProjectBranch } from "@/lib/types";

// TODO(7a): `GET /projects/{id}/branches` gains the latest visible handoff per branch.
export interface BranchHandoff {
  id: string;
  title: string;
  summary: string;
  fields?: Record<string, string | string[]>;
  updated_at: number;
  device_name?: string | null;
}
export type BranchRow = ProjectBranch & { handoff?: BranchHandoff | null };

function words(key: string): string {
  const text = key.replaceAll("_", " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function Handoff({ handoff }: { handoff: BranchHandoff }) {
  const [open, setOpen] = useState(false);
  const fields = Object.entries(handoff.fields ?? {});
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex items-start gap-1">
        <span className="min-w-0 flex-1 text-sm [overflow-wrap:anywhere]">{handoff.summary}</span>
        {fields.length > 0 ? (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-expanded={open}
            aria-label={open ? "Hide handoff details" : "Show handoff details"}
            onClick={() => setOpen((value) => !value)}
          >
            <ChevronRightIcon className={cn("transition-transform", open && "rotate-90")} />
          </Button>
        ) : null}
      </div>
      {open ? (
        <dl className="flex flex-col gap-1 text-sm">
          {fields.map(([key, value]) => (
            <Fragment key={key}>
              <dt className="text-xs font-medium text-muted-foreground">{words(key)}</dt>
              <dd className="[overflow-wrap:anywhere]">{Array.isArray(value) ? value.join(", ") : value}</dd>
            </Fragment>
          ))}
          {handoff.device_name ? <dd className="text-xs text-muted-foreground">Written on {handoff.device_name}</dd> : null}
        </dl>
      ) : null}
    </div>
  );
}

/** Read-only: where work stopped per branch. */
export function ProjectBranches({ branches }: { branches: BranchRow[] }) {
  if (branches.length === 0) {
    return (
      <Empty className="border">
        <EmptyHeader>
          <EmptyTitle>No branches yet</EmptyTitle>
          <EmptyDescription>Branches appear when an agent saves a memory from a branch of this repository.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <div className="min-w-0 overflow-hidden rounded-lg border bg-card">
      <Table aria-label="Branches">
        <TableHeader>
          <TableRow>
            <TableHead scope="col">Branch</TableHead>
            <TableHead scope="col">Memories</TableHead>
            <TableHead scope="col" className="hidden md:table-cell">Latest handoff</TableHead>
            <TableHead scope="col" className="hidden sm:table-cell">Updated</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {branches.map((branch) => (
            <TableRow key={branch.branch}>
              <TableCell className="align-top">
                <code className="font-mono text-xs break-all">{branch.branch}</code>
              </TableCell>
              <TableCell className="align-top tabular-nums">{branch.memories}</TableCell>
              <TableCell className="hidden align-top whitespace-normal md:table-cell">
                {branch.handoff ? <Handoff handoff={branch.handoff} /> : <span className="text-muted-foreground">—</span>}
              </TableCell>
              <TableCell className="hidden align-top sm:table-cell">
                <Time value={branch.last_updated} className="text-xs whitespace-nowrap text-muted-foreground" />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
