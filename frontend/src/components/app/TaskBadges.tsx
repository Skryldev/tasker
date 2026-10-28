import { IndicatorDot } from "@/components/ui/badge";
import { cn, PRIORITY_LABELS, STATUS_LABELS, type Priority, type TaskStatus } from "@/lib/utils";

const STATUS_DOT: Record<TaskStatus, string> = {
  backlog: "bg-status-backlog",
  todo: "bg-status-todo",
  in_progress: "bg-status-in-progress",
  in_review: "bg-status-in-review",
  done: "bg-status-done",
};

export function StatusBadge({ status, className }: { status: string; className?: string }) {
  const s = status as TaskStatus;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-0.5 text-2xs font-medium",
        className,
      )}
    >
      <IndicatorDot className={STATUS_DOT[s] ?? "bg-muted-foreground"} />
      {STATUS_LABELS[s] ?? status}
    </span>
  );
}

/** Three ascending bars; filled per priority level, red for urgent. */
export function PriorityBadge({ priority, className }: { priority: string; className?: string }) {
  const p = priority as Priority;
  const level: Record<Priority, number> = { low: 1, medium: 2, high: 3, urgent: 3 };
  const color: Record<Priority, string> = {
    low: "text-priority-low",
    medium: "text-priority-medium",
    high: "text-priority-high",
    urgent: "text-priority-urgent",
  };
  const filled = level[p] ?? 1;
  return (
    <span
      className={cn("inline-flex items-center gap-1.5 text-2xs font-medium", className)}
      title={`Priority: ${PRIORITY_LABELS[p] ?? priority}`}
    >
      <span className="flex items-end gap-px" aria-hidden>
        {[3, 5, 7].map((h, i) => (
          <span
            key={h}
            style={{ height: `${h}px` }}
            className={cn("w-0.5 rounded-sm", i < filled ? color[p] : "bg-border")}
          />
        ))}
      </span>
      {PRIORITY_LABELS[p] ?? priority}
    </span>
  );
}

export function StatusDot({ status, className }: { status: string; className?: string }) {
  return (
    <IndicatorDot
      className={cn(STATUS_DOT[status as TaskStatus] ?? "bg-muted-foreground", className)}
    />
  );
}
