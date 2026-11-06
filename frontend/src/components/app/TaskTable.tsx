import { useMemo, type ReactNode } from "react";
import type { Task } from "@/api/client";
import { useUserInfo } from "@/api/hooks";
import { Avatar } from "@/components/ui/avatar";
import { Checkbox } from "@/components/ui/checkbox";
import { PriorityBadge } from "./TaskBadges";
import { cn } from "@/lib/utils";
import { formatJalaliDate } from "@/lib/jalali";

/**
 * Phase 10: hand-rolled table. The previous implementation routed a static,
 * single-selection six-column table through @tanstack/table-core — a ~109 kB
 * (raw) runtime for zero derived behavior (no sorting, filtering, or grouped
 * row models were used). The markup, test ids, and interaction surface are
 * unchanged; the library is gone from the bundle.
 *
 * Visual spec (§5.1): 36px rows, status as dot + text, priority as icon,
 * avatar + truncated assignee with hover identity, relative due date with
 * the absolute Jalali date on hover, 100ms hover shift, 2px left-border
 * selection accent.
 */

const STATUS_DOT: Record<string, string> = {
  backlog: "bg-status-backlog",
  todo: "bg-status-todo",
  in_progress: "bg-status-in-progress",
  in_review: "bg-status-in-review",
  resolved: "bg-status-resolved",
  closed: "bg-status-closed",
};

/** Today in the product's calendar zone (Tehran) as an API date string. */
function tehranTodayISO(): string {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone: "Asia/Tehran",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(new Date());
}

function dayDiff(iso: string, todayISO: string): number {
  const a = Date.UTC(
    Number(iso.slice(0, 4)),
    Number(iso.slice(5, 7)) - 1,
    Number(iso.slice(8, 10)),
  );
  const b = Date.UTC(
    Number(todayISO.slice(0, 4)),
    Number(todayISO.slice(5, 7)) - 1,
    Number(todayISO.slice(8, 10)),
  );
  return Math.round((a - b) / 86_400_000);
}

/** Relative, color-coded due label; the absolute Jalali date is the hover. */
function DueCell({ value }: { value: string | null | undefined }) {
  if (!value) return <span className="text-xs text-muted-foreground">—</span>;
  const diff = dayDiff(value, tehranTodayISO());
  let label: string;
  let tone: string;
  if (diff < 0) {
    label = diff === -1 ? "1d overdue" : `${-diff}d overdue`;
    tone = "text-destructive";
  } else if (diff === 0) {
    label = "Due today";
    tone = "text-warning";
  } else if (diff === 1) {
    label = "Tomorrow";
    tone = "text-foreground";
  } else if (diff <= 3) {
    label = `in ${diff}d`;
    tone = "text-foreground";
  } else {
    label = `in ${diff}d`;
    tone = "text-muted-foreground";
  }
  return (
    <span className={cn("text-xs tabular-nums", tone)} title={`Due ${formatJalaliDate(value)}`}>
      {label}
    </span>
  );
}

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
  // One batched identity request powers every hover card in the table.
  const assigneeIds = useMemo(
    () => [...new Set(tasks.map((t) => t.assignee?.id).filter((id): id is string => Boolean(id)))],
    [tasks],
  );
  const identities = useUserInfo(assigneeIds);

  const hoverTitle = (task: Task): string | undefined => {
    const id = task.assignee?.id;
    if (!id) return undefined;
    const info = identities.data?.items.find((i) => i.id === id);
    if (!info) return task.assignee?.display_name;
    return [
      info.display_name,
      info.username ? `@${info.username}` : null,
      info.bio ?? null,
    ]
      .filter(Boolean)
      .join("\n");
  };

  const columns = ["Selected", "Task", "Status", "Priority", "Assignee", "Due"] as const;
  const skeletonRows = [0, 1, 2, 3];

  return (
    <div className="overflow-hidden rounded-lg border" data-testid="task-table">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr className="border-b bg-muted/50 text-left">
            {columns.map((c) => (
              <th
                key={c}
                className={cn(
                  "px-3 py-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground",
                  c === "Selected" && "sr-only",
                )}
              >
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {isLoading
            ? skeletonRows.map((i) => (
                <tr key={i} className="h-9 border-b last:border-0">
                  {columns.map((c) => (
                    <td key={c} className="px-3">
                      <div className="h-3.5 w-full max-w-24 animate-pulse rounded bg-muted" />
                    </td>
                  ))}
                </tr>
              ))
            : tasks.map((task) => {
                const selected = task.id === selectedId;
                return (
                  <tr
                    key={task.id}
                    tabIndex={0}
                    onClick={() => onSelect(task)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") onSelect(task);
                    }}
                    className={cn(
                      "h-9 cursor-pointer border-b transition-colors duration-100 last:border-0 hover:bg-accent/60 focus-visible:bg-accent focus-visible:outline-none",
                      selected &&
                        "border-l-2 border-l-primary bg-primary/5 hover:bg-primary/5 focus-visible:bg-primary/5",
                    )}
                    data-testid={`task-row-${task.id}`}
                    aria-selected={selected}
                  >
                    <td className="px-3">
                      <Checkbox checked={selected} aria-label="Selected task" />
                    </td>
                    <td className="max-w-[24rem] px-3">
                      <span className="flex items-center gap-2">
                        <span
                          className={cn("h-1.5 w-1.5 shrink-0 rounded-full", STATUS_DOT[task.status])}
                          aria-hidden
                        />
                        <span className="min-w-0">
                          <span className="block truncate font-medium leading-tight">
                            {task.title}
                          </span>
                          {renderContext ? (
                            <span className="block truncate text-2xs text-muted-foreground">
                              {renderContext(task)}
                            </span>
                          ) : null}
                        </span>
                        {task.labels.slice(0, 2).map((l) => (
                          <span
                            key={l.id}
                            className="hidden shrink-0 rounded-full border px-1.5 py-px text-2xs text-muted-foreground md:inline"
                          >
                            {l.name}
                          </span>
                        ))}
                      </span>
                    </td>
                    <td className="px-3">
                      <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                        <span
                          className={cn("h-1.5 w-1.5 shrink-0 rounded-full", STATUS_DOT[task.status])}
                          aria-hidden
                        />
                        {task.status.replaceAll("_", " ")}
                      </span>
                    </td>
                    <td className="px-3">
                      <PriorityBadge priority={task.priority} />
                    </td>
                    <td className="px-3">
                      {task.assignee ? (
                        <span
                          className="flex items-center gap-1.5 text-xs"
                          title={hoverTitle(task)}
                        >
                          <Avatar
                            name={task.assignee.display_name}
                            size="xs"
                            className="h-5 w-5"
                          />
                          <span className="hidden truncate lg:inline">
                            {task.assignee.display_name}
                          </span>
                        </span>
                      ) : (
                        <span className="text-xs text-muted-foreground">—</span>
                      )}
                    </td>
                    <td className="px-3">
                      <DueCell value={task.due_date} />
                    </td>
                  </tr>
                );
              })}
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
