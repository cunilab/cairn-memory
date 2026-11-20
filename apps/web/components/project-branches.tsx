import { formatTime } from "@/lib/report";
import type { ProjectBranch } from "@/lib/types";

/** Read-only: the branches the project's memories were saved on, newest activity first. */
export function ProjectBranches({ branches }: { branches: ProjectBranch[] }) {
  return (
    <section className="mt-8 flex flex-col gap-4">
      <div>
        <h2 className="font-semibold">Branches</h2>
        <p className="text-sm text-muted-foreground">
          Derived from the branch each project memory was saved on.
        </p>
      </div>
      {branches.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          Branches appear when a memory is saved from a branch of this repository.
        </p>
      ) : (
        branches.map((branch) => (
          <div
            key={branch.branch}
            className="flex flex-wrap items-center gap-3 rounded-md border p-3"
          >
            <span className="min-w-0 flex-1 text-sm break-all">
              {branch.branch}{" "}
              <span className="text-muted-foreground">
                · {branch.memories} {branch.memories === 1 ? "memory" : "memories"} · last{" "}
                {formatTime(branch.last_updated)}
              </span>
            </span>
          </div>
        ))
      )}
    </section>
  );
}
