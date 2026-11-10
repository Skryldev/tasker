import { Fragment, useMemo, useState, type ReactNode } from "react";
import { ChevronDown, Flag, ListChecks } from "lucide-react";
import type { Task } from "@/api/client";
import { useUserInfo } from "@/api/hooks";
import { Avatar } from "@/components/ui/avatar";
import { Checkbox } from "@/components/ui/checkbox";
import { PriorityBadge, StatusBadge } from "./TaskBadges";
import { LabelChip } from "./LabelChip";
import { cn, STATUS_LABELS, TASK_STATUSES, type TaskStatus } from "@/lib/utils";
import { formatJalaliDate } from "@/lib/jalali";

/**
 * Phase 10: hand-rolled table (the @tanstack/table-core runtime was dropped
 * for zero derived behavior). Phase 12 (WS8): rows group by status in
 * lifecycle order, each group collapsible; labels render as pastel chips,
 * deadlines as red flag chips, and subtask progress as a compact pill.
 * The markup contract (`task-row-{id}` test ids, selection, reorder) is
 * unchanged.
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

/** WS3: red-tinted deadline flag with the Jalali date on hover. */
function DeadlineCell({ value }: { value: string | null | undefined }) {
  if (!value) return null;
  const overdue = dayDiff(value, tehranTodayISO()) < 0;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-0.5 rounded-full border px-1.5 py-px text-2xs font-medium",
        overdue ? "border-destructive/60 bg-destructive/15 text-destructive" : "border-destructive/40 bg-destructive/10 text-destructive",
      )}
      title={`Deadline ${formatJalaliDate(value)}`}
      data-testid={`deadline-${value}`}
    >
      <Flag className="h-2.5 w-2.5" aria-hidden />
      {value.slice(5)}
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
  onReorder,
}: {
  tasks: Task[];
  total?: number;
  selectedId?: string;
  onSelect: (task: Task) => void;
  isLoading: boolean;
  /** Optional secondary line under the title (e.g. team/project context). */
  renderContext?: (task: Task) => ReactNode;
  /** Manual ordering (Phase 11): move `task` above `before` in its column. */
  onReorder?: (task: Task, beforeTaskId?: string) => void;
}) {
  // One batched identity request powers every hover card in the table.
  const assigneeIds = useMemo(
    () => [...new Set(tasks.map((t) => t.assignee?.id).filter((id): id is string => Boolean(id)))],
    [tasks],
  );
  const identities = useUserInfo(assigneeIds);

  // WS8: group rows by status in lifecycle order; groups collapse locally.
  const groups = useMemo(
    () =>
      TASK_STATUSES.map((status) => ({
        status,
        items: tasks.filter((t) => t.status === status),
      })).filter((g) => g.items.length > 0),
    [tasks],
  );
  const [collapsed, setCollapsed] = useState<Set<TaskStatus>>(new Set());
  const toggleGroup = (status: TaskStatus) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(status)) next.delete(status);
      else next.add(status);
      return next;
    });
  };

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

  const renderRow = (task: Task, groupTasks: Task[]) => {
    const selected = task.id === selectedId;
    return (
      <tr
        key={task.id}
        tabIndex={0}
        onClick={() => onSelect(task)}
        draggable={Boolean(onReorder)}
        onDragStart={(e) => {
          e.dataTransfer.setData("text/task-id", task.id);
          e.dataTransfer.effectAllowed = "move";
        }}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes("text/task-id")) e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          const draggedId = e.dataTransfer.getData("text/task-id");
          const dragged = groupTasks.find((t) => t.id === draggedId);
          if (dragged && dragged.id !== task.id && onReorder) {
            onReorder(dragged, task.id);
          }
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") onSelect(task);
          // Keyboard reordering (Phase 11): Alt+Up/Down moves the task above
          // or below its neighbor within the same status group.
          if (onReorder && e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
            e.preventDefault();
            const idx = groupTasks.findIndex((t) => t.id === task.id);
            const neighbor = groupTasks[idx + (e.key === "ArrowUp" ? -1 : 1)];
            if (neighbor) {
              if (e.key === "ArrowUp") {
                onReorder(task, neighbor.id);
              } else {
                onReorder(neighbor, task.id);
              }
            }
          }
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
            {/* WS4: compact subtask progress pill. */}
            {task.subtasks && task.subtasks.length > 0 ? (
              <span
                className="hidden shrink-0 items-center gap-1 rounded-full bg-muted px-1.5 py-px text-2xs tabular-nums text-muted-foreground md:inline-flex"
                title={`${task.subtasks.filter((s) => s.status === "closed").length} of ${task.subtasks.length} subtasks closed`}
              >
                <ListChecks className="h-2.5 w-2.5" aria-hidden />
                {task.subtasks.filter((s) => s.status === "closed").length}/{task.subtasks.length}
              </span>
            ) : null}
            {/* WS5: pastel label chips tinted with the label color. */}
            {task.labels.slice(0, 2).map((l) => (
              <LabelChip
                key={l.id}
                name={l.name}
                color={l.color}
                icon={l.icon}
                className="hidden md:inline-flex max-w-[6rem]"
              />
            ))}
          </span>
        </td>
        <td className="px-3">
          <StatusBadge status={task.status} />
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
          <span className="flex items-center gap-1.5">
            <DueCell value={task.due_date} />
            <DeadlineCell value={task.deadline} />
          </span>
        </td>
      </tr>
    );
  };

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
            : groups.map(({ status, items }) => (
                <Fragment key={`group-${status}`}>
                  <tr className="border-b bg-muted/30">
                    <td colSpan={columns.length} className="px-3 py-1">
                      <button
                        type="button"
                        onClick={() => toggleGroup(status)}
                        aria-expanded={!collapsed.has(status)}
                        className="flex items-center gap-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        data-testid={`task-group-${status}`}
                      >
                        <ChevronDown
                          className={cn(
                            "h-3 w-3 transition-transform",
                            collapsed.has(status) && "-rotate-90",
                          )}
                          aria-hidden
                        />
                        <span className={cn("h-1.5 w-1.5 rounded-full", STATUS_DOT[status])} aria-hidden />
                        {STATUS_LABELS[status]}
                        <span className="tabular-nums">({items.length})</span>
                      </button>
                    </td>
                  </tr>
                  {!collapsed.has(status) ? items.map((t) => renderRow(t, items)) : null}
                </Fragment>
              ))}
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
