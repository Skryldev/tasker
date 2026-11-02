import { useMemo, type ReactNode } from "react";
import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  useReactTable,
  type RowSelectionState,
} from "@tanstack/react-table";
import type { Task } from "@/api/client";
import { Avatar } from "@/components/ui/avatar";
import { Checkbox } from "@/components/ui/checkbox";
import { PriorityBadge, StatusBadge, StatusDot } from "./TaskBadges";
import { cn } from "@/lib/utils";
import { formatJalaliDate } from "@/lib/doran";

const columnHelper = createColumnHelper<Task>();

export function TaskTable({
  tasks,
  total,
  selectedId,
  onSelect,
  isLoading,
  renderContext,
}: {
  tasks: Task[];
  total?: number;
  selectedId?: string;
  onSelect: (task: Task) => void;
  isLoading: boolean;
  /** Optional secondary line under the title (e.g. team/project context). */
  renderContext?: (task: Task) => ReactNode;
}) {
  const selection = useMemo<RowSelectionState>(() => {
    const found = tasks.find((t) => t.id === selectedId);
    return found ? { [found.id]: true } : {};
  }, [tasks, selectedId]);

  const columns = useMemo(
    () => [
      columnHelper.display({
        id: "select",
        header: () => <span className="sr-only">Selected</span>,
        cell: ({ row }) => (
          <Checkbox checked={Boolean(selection[row.original.id])} aria-label="Selected task" />
        ),
      }),
      columnHelper.accessor("title", {
        header: "Task",
        cell: ({ row }) => (
          <span className="flex items-center gap-2">
            <StatusDot status={row.original.status} />            <span className="min-w-0">
              <span className="block truncate font-medium">{row.original.title}</span>
              {renderContext ? (
                <span className="block truncate text-2xs text-muted-foreground">
                  {renderContext(row.original)}
                </span>
              ) : null}
            </span>
            {row.original.labels.slice(0, 2).map((l) => (
              <span key={l.id} className="hidden shrink-0 rounded-full border px-1.5 py-px text-2xs text-muted-foreground md:inline">
                {l.name}
              </span>
            ))}
          </span>
        ),
      }),
      columnHelper.accessor("status", {
        header: "Status",
        cell: ({ getValue }) => <StatusBadge status={getValue()} />,
      }),
      columnHelper.accessor("priority", {
        header: "Priority",
        cell: ({ getValue }) => <PriorityBadge priority={getValue()} />,
      }),
      columnHelper.accessor((row) => row.assignee?.display_name ?? "", {
        id: "assignee",
        header: "Assignee",
        cell: ({ row }) =>
          row.original.assignee ? (
            <span className="flex items-center gap-1.5 text-xs">
              <Avatar name={row.original.assignee.display_name} size="xs" />
              <span className="hidden truncate lg:inline">{row.original.assignee.display_name}</span>
            </span>
          ) : (
            <span className="text-xs text-muted-foreground">—</span>
          ),
      }),
      columnHelper.accessor("due_date", {
        header: "Due",
        cell: ({ getValue }) => (
          <span className="text-xs text-muted-foreground">{formatJalaliDate(getValue())}</span>
        ),
      }),
    ],
    [selection],
  );

  const table = useReactTable({
    data: tasks,
    columns,
    getCoreRowModel: getCoreRowModel(),
    state: { rowSelection: selection },
    getRowId: (row) => row.id,
    enableRowSelection: true,
  });

  return (
    <div className="overflow-hidden rounded-lg border" data-testid="task-table">
      <table className="w-full border-collapse text-sm">
        <thead>
          {table.getHeaderGroups().map((headerGroup) => (
            <tr key={headerGroup.id} className="border-b bg-muted/50 text-left">
              {headerGroup.headers.map((header) => (
                <th key={header.id} className="px-3 py-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {header.isPlaceholder ? null : flexRender(header.column.columnDef.header, header.getContext())}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {isLoading ? (
            [0, 1, 2, 3].map((i) => (
              <tr key={i} className="border-b last:border-0">
                {columns.map((_, idx) => (
                  <td key={idx} className="px-3 py-2.5">
                    <div className="h-4 w-full animate-pulse rounded bg-muted" />
                  </td>
                ))}
              </tr>
            ))
          ) : (
            table.getRowModel().rows.map((row) => (
              <tr
                key={row.id}
                tabIndex={0}
                onClick={() => onSelect(row.original)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") onSelect(row.original);
                }}
                className={cn(
                  "cursor-pointer border-b transition-colors last:border-0 hover:bg-accent/60 focus-visible:bg-accent focus-visible:outline-none",
                  row.id === selectedId && "bg-primary/5",
                )}
                data-testid={`task-row-${row.id}`}
                aria-selected={row.id === selectedId}
              >
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id} className="max-w-[24rem] px-3 py-2">
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
      {total !== undefined && total > tasks.length ? (
        <div className="border-t bg-muted/30 px-3 py-1.5 text-2xs text-muted-foreground">
          Showing {tasks.length} of {total} tasks
        </div>
      ) : null}
    </div>
  );
}
