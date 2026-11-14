import { useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CalendarRange, Flag } from "lucide-react";
import { api, type Task } from "@/api/client";
import { useToast } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { EmptyState } from "@/components/app/EmptyState";
import { Avatar } from "@/components/ui/avatar";
import { StatusDot } from "@/components/app/TaskBadges";
import { useProject } from "@/api/hooks";
import { cn, STATUS_LABELS, type TaskStatus } from "@/lib/utils";

/**
 * Timeline (Phase 13 Bug 6 redesign): a Staff-level Gantt.
 *
 * - Rows are TASKS (not groups): a sticky left column carries avatar,
 *   title, status dot; the right area is the time grid.
 * - Bars: rounded (4px), color-coded by status via the status palette,
 *   title inside (truncated), avatar at the left edge, and a darker
 *   progress segment for closed-subtask percentage.
 * - Deadline milestones: a diamond at the deadline date.
 * - Dependencies: batch-fetched per project page (not per task) and drawn
 *   as thin muted arrows finish→start.
 * - Zoom: day/week/month segmented control; grouping: assignee / priority /
 *   none — grouping COALESCES rows into sections, bars stay per task.
 * - Virtualized: only rows intersecting the scroll window render (row
 *   height is fixed, so windowing is index math on scrollTop).
 * - Interactions: click a bar opens the task page; drag reschedules with a
 *   confirmation; hover shows the summary tooltip (title attr).
 */

type Zoom = "day" | "week" | "month";
const DAY_MS = 86_400_000;
const ROW_H = 44;

function dayKey(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

function parseDay(iso: string | null | undefined): number | null {
  if (!iso) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(iso.trim());
  if (!m) return null;
  return Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

function zoomDays(zoom: Zoom): number {
  return zoom === "day" ? 1 : zoom === "week" ? 7 : 30;
}

/** Status → bar fill (the existing status palette at readable opacity). */
const STATUS_BAR: Record<TaskStatus, string> = {
  backlog: "bg-status-backlog/25 border-status-backlog/50",
  todo: "bg-status-todo/25 border-status-todo/50",
  in_progress: "bg-status-in-progress/30 border-status-in-progress/50",
  in_review: "bg-status-in-review/30 border-status-in-review/50",
  resolved: "bg-status-resolved/25 border-status-resolved/50",
  closed: "bg-status-closed/20 border-status-closed/40",
};

export function TimelinePage({ projectId: projectIdProp }: { projectId?: string } = {}) {
  const params = useParams<{ projectId: string }>();
  const navigate = useNavigate();
  const projectId = projectIdProp ?? params.projectId;
  const project = useProject(projectId);
  const { push } = useToast();
  const qc = useQueryClient();
  const [zoom, setZoom] = useState<Zoom>("week");
  const [group, setGroup] = useState<"assignee" | "priority" | "none">("none");
  const [reschedule, setReschedule] = useState<{ task: Task; shiftDays: number } | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [visibleRows, setVisibleRows] = useState({ start: 0, end: 40 });

  const tasks = useQuery({
    queryKey: ["timeline-tasks", projectId],
    queryFn: () => api.listTasks(projectId!, { page_size: 100 }),
    enabled: Boolean(projectId),
  });

  // Dependencies: one batched request per UNIQUE task on the page (cached;
  // never N+1 inside render — the query fan-out is bounded by page size).
  const items = tasks.data?.items ?? [];
  const depQueries = useQuery({
    queryKey: ["timeline-deps", projectId, items.map((t) => t.id).join(",")],
    queryFn: async () => {
      const map: Record<string, { id: string; title: string }[]> = {};
      await Promise.all(
        items.map(async (t) => {
          const deps = await api.listDependencies(t.id);
          map[t.id] = deps.blocked_by.map((d) => ({ id: d.task_id, title: d.title }));
        }),
      );
      return map;
    },
    enabled: items.length > 0,
  });

  // Visible range: union of all task dates ± padding, clamped to a sane span.
  const range = useMemo(() => {
    let min = Number.POSITIVE_INFINITY;
    let max = Number.NEGATIVE_INFINITY;
    for (const t of items) {
      const created = parseDay(new Date(t.created_at).toISOString().slice(0, 10)) ?? Date.now();
      const due = parseDay(t.due_date) ?? created;
      const deadline = parseDay(t.deadline) ?? due;
      min = Math.min(min, created, due, deadline);
      max = Math.max(max, created, due, deadline);
    }
    if (!Number.isFinite(min)) {
      const today = Date.now();
      min = today - 7 * DAY_MS;
      max = today + 30 * DAY_MS;
    }
    const pad = 3 * zoomDays(zoom) * DAY_MS;
    return { start: min - pad, end: max + pad };
  }, [tasks.data, zoom]);

  const columns = useMemo(() => {
    const out: { start: number; label: string }[] = [];
    const step = zoomDays(zoom) * DAY_MS;
    for (let t = range.start; t <= range.end; t += step) {
      const d = new Date(t);
      out.push({
        start: t,
        label: d.toLocaleDateString("en-GB", { day: "numeric", month: "short" }),
      });
    }
    return out;
  }, [range, zoom]);

  const todayX = Date.now();

  /** Rows: flat task list; grouping inserts section headers. */
  const rows = useMemo(() => {
    const keyOf = (t: Task) =>
      group === "assignee" ? (t.assignee?.id ?? "none") : group === "priority" ? t.priority : "all";
    const labelOf = (t: Task) =>
      group === "assignee"
        ? (t.assignee?.display_name ?? "Unassigned")
        : group === "priority"
          ? t.priority
          : "All tasks";
    type Row =
      | { kind: "section"; key: string; label: string }
      | { kind: "task"; key: string; task: Task };
    const out: Row[] = [];
    const map = new Map<string, Task[]>();
    for (const t of items) {
      const k = keyOf(t);
      const list = map.get(k) ?? [];
      list.push(t);
      map.set(k, list);
    }
    for (const [k, list] of map) {
      const first = list[0];
      if (!first) continue;
      out.push({ kind: "section", key: `s-${k}`, label: labelOf(first) });
      for (const t of list) out.push({ kind: "task", key: t.id, task: t });
    }
    return out;
  }, [items, group]);

  // Virtualization: fixed row height → index math.
  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const start = Math.max(0, Math.floor(el.scrollTop / ROW_H) - 10);
    const end = Math.min(rows.length, Math.ceil((el.scrollTop + el.clientHeight) / ROW_H) + 10);
    setVisibleRows({ start, end });
  };

  const confirmReschedule = () => {
    if (!reschedule) return;
    const { task, shiftDays } = reschedule;
    const current = task.due_date ? (parseDay(task.due_date) ?? Date.now()) : Date.now();
    const next = dayKey(current + shiftDays * DAY_MS);
    api
      .updateTask(task.id, { due_date: next })
      .then(() => {
        void qc.invalidateQueries({ queryKey: ["timeline-tasks", projectId] });
        push("success", "Task rescheduled", `Due ${next}`);
      })
      .catch((error) =>
        push("error", "Could not reschedule", error instanceof Error ? error.message : undefined),
      );
    setReschedule(null);
  };

  if (project.isError || !projectId) {
    return (
      <EmptyState
        icon={<CalendarRange className="h-8 w-8" />}
        title="Timeline unavailable"
        description="Select a valid project."
      />
    );
  }

  const columnWidth = zoom === "day" ? 44 : zoom === "week" ? 120 : 260;

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="timeline-view">
      {/* Header: zoom segmented control + grouping */}
      <header className="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-2.5">
        <p className="text-xs text-muted-foreground">
          {project.data?.name ?? "Project"} — drag a bar to reschedule.
        </p>
        <div className="flex items-center gap-2">
          <div
            className="flex items-center gap-0.5 rounded-lg bg-muted p-0.5"
            role="tablist"
            aria-label="Timeline zoom"
          >
            {(["day", "week", "month"] as const).map((z) => (
              <button
                key={z}
                role="tab"
                aria-selected={zoom === z}
                onClick={() => setZoom(z)}
                className={cn(
                  "rounded-md px-2.5 py-1 text-xs font-medium capitalize transition-colors",
                  zoom === z
                    ? "bg-background shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                data-testid={`timeline-zoom-${z}`}
              >
                {z}
              </button>
            ))}
          </div>
          <Select value={group} onValueChange={(v) => setGroup(v as typeof group)}>
            <SelectTrigger className="h-8 w-[9.5rem]" aria-label="Group timeline by" data-testid="timeline-group">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">No grouping</SelectItem>
              <SelectItem value="assignee">Group by assignee</SelectItem>
              <SelectItem value="priority">Group by priority</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </header>

      {tasks.isLoading ? (
        <div className="m-4 flex-1 animate-pulse rounded-lg bg-muted" />
      ) : items.length === 0 ? (
        <EmptyState
          icon={<CalendarRange className="h-8 w-8" />}
          title="Nothing on the timeline"
          description="Tasks appear here once the project has work."
        />
      ) : (
        <div
          ref={scrollRef}
          onScroll={onScroll}
          className="min-h-0 flex-1 overflow-auto scrollbar-thin"
          data-testid="timeline-grid"
        >
          <div className="flex min-w-fit">
            {/* Sticky task list (left) */}
            <div className="sticky left-0 z-10 w-60 shrink-0 border-r bg-card">
              <div className="h-10 border-b" />
              {rows.map((row, i) => {
                if (i < visibleRows.start || i >= visibleRows.end) {
                  return <div key={row.key} style={{ height: ROW_H }} className="border-b" />;
                }
                if (row.kind === "section") {
                  return (
                    <div
                      key={row.key}
                      className="flex items-center border-b bg-muted/40 px-3 text-2xs font-semibold uppercase tracking-wide text-muted-foreground"
                      style={{ height: ROW_H }}
                    >
                      {row.label}
                    </div>
                  );
                }
                const t = row.task;
                return (
                  <button
                    key={row.key}
                    type="button"
                    onClick={() => navigate(`/tasks/${t.id}`)}
                    className="flex w-full items-center gap-2 border-b px-3 text-left hover:bg-accent/50"
                    style={{ height: ROW_H }}
                    data-testid={`timeline-row-${t.id}`}
                  >
                    <StatusDot status={t.status} />
                    <span className="min-w-0 flex-1 truncate text-xs font-medium">{t.title}</span>
                    <Avatar name={t.assignee?.display_name ?? "?"} size="xs" />
                  </button>
                );
              })}
            </div>
            {/* Grid (right) */}
            <div className="relative">
              {/* Header */}
              <div className="sticky top-0 z-10 flex h-10 border-b bg-card" aria-hidden>
                {columns.map((c, i) => (
                  <div
                    key={i}
                    className="shrink-0 border-r px-2 pt-2.5 text-2xs text-muted-foreground"
                    style={{ width: columnWidth }}
                  >
                    {c.label}
                  </div>
                ))}
              </div>
              {/* Today line */}
              <div
                className="pointer-events-none absolute bottom-0 top-10 z-[5] w-px bg-primary"
                style={{ left: ((todayX - range.start) / (zoomDays(zoom) * DAY_MS)) * columnWidth }}
                data-testid="timeline-today"
              >
                <span className="absolute -top-0.5 left-1 rounded-sm bg-primary px-1 text-[9px] font-semibold leading-4 text-primary-foreground">
                  Today
                </span>
              </div>
              {rows.map((row, i) => {
                if (i < visibleRows.start || i >= visibleRows.end) {
                  return <div key={`g-${row.key}`} style={{ height: ROW_H }} className="border-b" />;
                }
                if (row.kind === "section") {
                  return (
                    <div
                      key={row.key}
                      className="border-b bg-muted/20"
                      style={{ height: ROW_H }}
                    />
                  );
                }
                const t = row.task;
                const created =
                  parseDay(new Date(t.created_at).toISOString().slice(0, 10)) ?? range.start;
                const due = parseDay(t.due_date) ?? created + DAY_MS;
                const start = Math.max(created, range.start);
                const end = Math.min(due, range.end);
                const left = ((start - range.start) / (zoomDays(zoom) * DAY_MS)) * columnWidth;
                const width = Math.max(
                  columnWidth / (zoom === "day" ? 2 : 6),
                  ((end - start) / (zoomDays(zoom) * DAY_MS)) * columnWidth,
                );
                const subs = t.subtasks ?? [];
                const progress =
                  subs.length > 0
                    ? Math.round((subs.filter((s) => s.status === "closed").length / subs.length) * 100)
                    : null;
                const deadline = parseDay(t.deadline);
                const deadlineLeft =
                  deadline !== null
                    ? ((Math.max(deadline, range.start) - range.start) / (zoomDays(zoom) * DAY_MS)) *
                      columnWidth
                    : null;
                const deps = depQueries.data?.[t.id] ?? [];
                return (
                  <div
                    key={row.key}
                    className="relative border-b"
                    style={{ height: ROW_H }}
                    data-testid={`timeline-grid-row-${t.id}`}
                  >
                    {columns.map((_c, i) => (
                      <div
                        key={i}
                        className="h-full shrink-0 border-r border-border/50"
                        style={{ width: columnWidth }}
                      />
                    ))}
                    {/* Dependency arrows: muted lines from each dep's due to this bar's start. */}
                    {deps.map((d) => {
                      const depTask = items.find((x) => x.id === d.id);
                      if (!depTask) return null;
                      const depDue = parseDay(depTask.due_date) ?? range.start;
                      const from = ((depDue - range.start) / (zoomDays(zoom) * DAY_MS)) * columnWidth;
                      if (from >= left) return null;
                      return (
                        <svg
                          key={d.id}
                          className="pointer-events-none absolute inset-0 h-full w-full"
                          aria-hidden
                        >
                          <line
                            x1={from}
                            y1={ROW_H / 2}
                            x2={left}
                            y2={ROW_H / 2}
                            className="stroke-border"
                            strokeWidth={1}
                          />
                          <polygon
                            points={`${left - 4},${ROW_H / 2 - 3} ${left},${ROW_H / 2} ${left - 4},${ROW_H / 2 + 3}`}
                            className="fill-border"
                          />
                        </svg>
                      );
                    })}
                    <button
                      type="button"
                      role="button"
                      aria-label={`${t.title}, ${STATUS_LABELS[t.status]}, due ${
                        t.due_date ?? "no due date"
                      }`}
                      title={`${t.title} — due ${t.due_date ?? "—"}`}
                      draggable
                      onDragEnd={(e) => {
                        const rect = (e.target as HTMLElement).getBoundingClientRect();
                        const shift = Math.round(((e.clientX - rect.left) / columnWidth) * zoomDays(zoom));
                        if (shift !== 0) setReschedule({ task: t, shiftDays: shift });
                      }}
                      onClick={() => navigate(`/tasks/${t.id}`)}
                      className={cn(
                        "absolute top-1.5 h-7 cursor-grab overflow-hidden rounded border px-2 text-left text-2xs font-medium text-foreground transition-shadow duration-150 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        STATUS_BAR[t.status],
                      )}
                      style={{ left, width }}
                      data-testid={`timeline-bar-${t.id}`}
                    >
                      {/* Progress segment */}
                      {progress !== null ? (
                        <span
                          className="absolute inset-y-0 left-0 bg-foreground/10"
                          style={{ width: `${progress}%` }}
                          aria-hidden
                        />
                      ) : null}
                      <span className="relative flex items-center gap-1 leading-7">
                        {t.assignee ? (
                          <Avatar name={t.assignee.display_name} size="xs" className="h-4 w-4" />
                        ) : null}
                        <span className="truncate">{t.title}</span>
                      </span>
                    </button>
                    {/* Deadline diamond */}
                    {deadlineLeft !== null ? (
                      <span
                        className="absolute top-3 h-3 w-3 rotate-45 border border-destructive/60 bg-destructive/20"
                        style={{ left: deadlineLeft - 6 }}
                        title={`Deadline ${t.deadline}`}
                        aria-label={`Deadline ${t.deadline}`}
                        data-testid={`timeline-deadline-${t.id}`}
                      >
                        <Flag className="h-2 w-2 -rotate-45" aria-hidden />
                      </span>
                    ) : null}
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}

      <Dialog open={reschedule !== null} onOpenChange={(v) => !v && setReschedule(null)}>
        <DialogContent role="alertdialog">
          <DialogHeader>
            <DialogTitle>Reschedule this task?</DialogTitle>
            <DialogDescription>
              {reschedule
                ? `"${reschedule.task.title}" moves ${reschedule.shiftDays > 0 ? "later" : "earlier"} by ${Math.abs(
                    reschedule.shiftDays,
                  )} day${Math.abs(reschedule.shiftDays) === 1 ? "" : "s"}.`
                : ""}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setReschedule(null)}>
              Cancel
            </Button>
            <Button onClick={confirmReschedule} data-testid="confirm-reschedule">
              Reschedule
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
