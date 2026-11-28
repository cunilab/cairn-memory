"use client";

const SIZES = [25, 50, 100] as const;

/** Numbered pager. `page` is 1-based and already clamped by the server. */
export function PageNav({
  page,
  pageSize,
  total,
  onPage,
  onPageSize,
}: {
  page: number;
  pageSize: number;
  total: number;
  onPage: (page: number) => void;
  onPageSize: (size: number) => void;
}) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  const start = total === 0 ? 0 : (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, total);
  const numbers = windowAround(page, pages);
  return (
    <nav aria-label="Pagination" className="flex flex-wrap items-center justify-between gap-3 text-sm">
      <p className="tabular-nums text-muted-foreground">
        {start}–{end} of {total}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2 text-muted-foreground">
          <span className="sr-only">Page size</span>
          <select
            className="h-11 rounded-md border bg-background px-2"
            value={pageSize}
            onChange={(event) => onPageSize(Number(event.target.value))}
          >
            {SIZES.map((size) => (
              <option key={size} value={size}>
                {size} per page
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="min-h-11 min-w-11 rounded-md border px-2 disabled:opacity-40" aria-label="Previous page" disabled={page <= 1} onClick={() => onPage(page - 1)}>
          ‹
        </button>
        {numbers.map((number, index) =>
          number === null ? (
            <span key={`gap-${index}`} className="px-1 text-muted-foreground" aria-hidden>
              …
            </span>
          ) : (
            <button
              key={number}
              type="button"
              className="min-h-11 min-w-11 rounded-md border px-2 tabular-nums aria-[current=page]:bg-primary aria-[current=page]:text-primary-foreground"
              aria-label={`Page ${number}`}
              aria-current={number === page ? "page" : undefined}
              onClick={() => onPage(number)}
            >
              {number}
            </button>
          ),
        )}
        <button type="button" className="min-h-11 min-w-11 rounded-md border px-2 disabled:opacity-40" aria-label="Next page" disabled={page >= pages} onClick={() => onPage(page + 1)}>
          ›
        </button>
      </div>
    </nav>
  );
}

function windowAround(page: number, pages: number): Array<number | null> {
  if (pages <= 7) return Array.from({ length: pages }, (_, index) => index + 1);
  const items: Array<number | null> = [1];
  const start = Math.max(2, page - 1);
  const end = Math.min(pages - 1, page + 1);
  if (start > 2) items.push(null);
  for (let number = start; number <= end; number += 1) items.push(number);
  if (end < pages - 1) items.push(null);
  items.push(pages);
  return items;
}
