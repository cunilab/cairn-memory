"use client";

import { Suspense, useState, type ReactNode } from "react";
import {
  flexRender,
  getCoreRowModel,
  getFilteredRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type RowData,
  type SortingState,
} from "@tanstack/react-table";
import { ArrowDownIcon, ArrowUpDownIcon, ArrowUpIcon, SearchIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { cn } from "@/lib/utils";

declare module "@tanstack/react-table" {
  // className on a column lets it hide below md so phones keep one readable column.
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface ColumnMeta<TData extends RowData, TValue> {
    className?: string;
  }
}

type DataTableProps<T> = {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  columns: ColumnDef<T, any>[];
  data: T[];
  /** Accessible name for the table and the filter box. */
  label: string;
  filterPlaceholder: string;
  empty: ReactNode;
  initialSort?: SortingState;
  pageSize?: number;
  toolbar?: ReactNode;
  getRowId?: (row: T) => string;
  /** Prefills the filter box. A branch link uses this; it is not a session citation. */
  initialFilter?: string;
  /** False when the caller filters (for example from the URL) and the box would duplicate it. */
  searchable?: boolean;
};

/** Sortable, filterable, paged table (shadcn data-table pattern on TanStack Table). */
export function DataTable<T>(props: DataTableProps<T>) {
  // TanStack Table calls Date.now() while building rows in development. That blocks
  // Cache Components prerender unless the table is under Suspense.
  return (
    <Suspense
      fallback={
        <DataTablePending
          label={props.label}
          filterPlaceholder={props.filterPlaceholder}
          empty={props.empty}
          searchable={props.searchable}
        />
      }
    >
      <DataTableReady {...props} />
    </Suspense>
  );
}

function DataTablePending({
  label,
  filterPlaceholder,
  empty,
  searchable = true,
}: Pick<DataTableProps<unknown>, "label" | "filterPlaceholder" | "empty" | "searchable">) {
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {searchable ? (
        <InputGroup className="w-full sm:max-w-xs">
          <InputGroupAddon>
            <SearchIcon />
          </InputGroupAddon>
          <InputGroupInput
            type="search"
            aria-label={`Filter ${label}`}
            placeholder={filterPlaceholder}
            disabled
          />
        </InputGroup>
      ) : null}
      <div className="min-w-0 overflow-hidden rounded-lg border bg-card">{empty}</div>
    </div>
  );
}

function DataTableReady<T>({
  columns,
  data,
  label,
  filterPlaceholder,
  empty,
  initialSort = [],
  pageSize = 10,
  toolbar,
  getRowId,
  searchable = true,
  initialFilter = "",
}: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(initialSort);
  const [filter, setFilter] = useState(initialFilter);
  // TanStack Table returns unstable function references; nothing here is memoized on them.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({
    data,
    columns,
    getRowId,
    state: { sorting, globalFilter: filter },
    onSortingChange: setSorting,
    onGlobalFilterChange: setFilter,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    initialState: { pagination: { pageSize } },
  });
  const rows = table.getRowModel().rows;
  const total = table.getFilteredRowModel().rows.length;
  const { pageIndex } = table.getState().pagination;
  const first = pageIndex * pageSize + 1;

  return (
    <div className="flex min-w-0 flex-col gap-3">
      {searchable || toolbar ? (
        <div className="flex flex-wrap items-center gap-2">
          {searchable ? (
            <InputGroup className="w-full sm:max-w-xs">
              <InputGroupAddon>
                <SearchIcon />
              </InputGroupAddon>
              <InputGroupInput
                type="search"
                aria-label={`Filter ${label}`}
                placeholder={filterPlaceholder}
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
              />
            </InputGroup>
          ) : null}
          {toolbar}
        </div>
      ) : null}
      <div className="min-w-0 overflow-hidden rounded-lg border bg-card">
        <Table aria-label={label}>
          <TableHeader>
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id} className="hover:bg-transparent">
                {group.headers.map((header) => {
                  const sorted = header.column.getIsSorted();
                  return (
                    <TableHead
                      key={header.id}
                      scope="col"
                      aria-sort={
                        sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined
                      }
                      className={cn("h-9 text-xs font-medium text-muted-foreground", header.column.columnDef.meta?.className)}
                    >
                      {header.isPlaceholder ? null : header.column.getCanSort() ? (
                        <button
                          type="button"
                          onClick={header.column.getToggleSortingHandler()}
                          className="-mx-1.5 inline-flex items-center gap-1 rounded-md px-1.5 py-1 outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
                        >
                          {flexRender(header.column.columnDef.header, header.getContext())}
                          {sorted === "asc" ? (
                            <ArrowUpIcon className="size-3" aria-hidden />
                          ) : sorted === "desc" ? (
                            <ArrowDownIcon className="size-3" aria-hidden />
                          ) : (
                            <ArrowUpDownIcon className="size-3 opacity-50" aria-hidden />
                          )}
                        </button>
                      ) : (
                        flexRender(header.column.columnDef.header, header.getContext())
                      )}
                    </TableHead>
                  );
                })}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {rows.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={columns.length} className="h-24 text-center text-muted-foreground">
                  {data.length === 0 ? empty : "Nothing matches this filter."}
                </TableCell>
              </TableRow>
            ) : (
              rows.map((row) => (
                <TableRow key={row.id}>
                  {row.getVisibleCells().map((cell) => (
                    <TableCell
                      key={cell.id}
                      className={cn("align-top whitespace-normal", cell.column.columnDef.meta?.className)}
                    >
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </TableCell>
                  ))}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
      {total > pageSize ? (
        <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
          <span aria-live="polite">
            {first}–{Math.min(first + pageSize - 1, total)} of {total}
          </span>
          <div className="flex gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={!table.getCanPreviousPage()}
              onClick={() => table.previousPage()}
            >
              Previous
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={!table.getCanNextPage()}
              onClick={() => table.nextPage()}
            >
              Next
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
