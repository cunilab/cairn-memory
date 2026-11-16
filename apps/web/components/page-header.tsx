import type { ReactNode } from "react";

/** Page title row: the only h1 on a page. */
export function PageHeader({
  title,
  description,
  actions,
}: {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
      <div className="flex min-w-0 flex-col gap-1">
        <h1 className="text-xl leading-tight font-semibold break-words">{title}</h1>
        {description ? (
          <p className="max-w-prose text-sm text-muted-foreground [overflow-wrap:anywhere]">
            {description}
          </p>
        ) : null}
      </div>
      {actions ? <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div> : null}
    </div>
  );
}
