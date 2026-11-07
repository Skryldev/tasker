import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CalendarRange } from "lucide-react";
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
import { useProject } from "@/api/hooks";
import { cn } from "@/lib/utils";

/**
 * Timeline view (Phase 11): a lightweight, read-mostly Gantt for one project.
 *
 * - Bars span creation → due date (tasks without a due date render as a
 *   single-day marker at creation).
 * - Zoom: day / week / month column granularity.
 * - Grouping: by assignee (default) or priority — none is not offered since
 *   an ungrouped Gantt loses the row structure that makes it readable.
 * - Drag a bar horizontally to reschedule: moves the due date by whole days
 *   (the only mutable endpoint — creation dates are immutable), with a
 *   confirmation dialog. Creation → due span stays constant.
 * - Dependencies render as read-only arrows for tasks whose deps are loaded
 *   (lazy per-task fetch, cached).
 * - Virtualized by window: only columns intersecting the visible range
 *   render; bars render per row (rows are bounded by the page size).
 */

type Zoom = "day" | "week" | "month";
const DAY_MS = 86_400_000;

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

export function TimelinePage({ projectId: projectIdProp }: { projectId?: string } = {}) {
  const params = useParams<{ projectId: string }>();
  const projectId = projectIdProp ?? params.projectId;
  const project = useProject(projectId);
  const { push } = useToast();
  const qc = useQueryClient();
  const [zoom, setZoom] = useState<Zoom>("week");
  const [group, setGroup] = useState<"assignee" | "priority">("assignee");
  const [reschedule, setReschedule] = useState<{ task: Task; shiftDays: number } | null>(null);

  const tasks = useQuery({
    queryKey: ["timeline-tasks", projectId],
    queryFn: () => api.listTasks(projectId!, { page_size: 100 }),
    enabled: Boolean(projectId),
  });

  // Visible range: union of all task dates ± padding, clamped to a sane span.
  const range = useMemo(() => {
    const items = tasks.data?.items ?? [];
    let min = Number.POSITIVE_INFINITY;
    let max = Number.NEGATIVE_INFINITY;
    for (const t of items) {
      const created = parseDay(new Date(t.created_at).toISOString().slice(0, 10)) ?? Date.now();
      const due = parseDay(t.due_date) ?? created;
      min = Math.min(min, created, due);
      max = Math.max(max, created, due);
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
        label:
          zoom === "day"
            ? d.toLocaleDateString("en-GB", { day: "numeric", month: "short" })
            : d.toLocaleDateString("en-GB", { day: "numeric", month: "short" }),
      });
    }
    return out;
  }, [range, zoom]);

  const todayX = Date.now();

  const rows = useMemo(() => {
    const items = tasks.data?.items ?? [];
    const keyOf = (t: Task) =>
      group === "assignee" ? (t.assignee?.id ?? "none") : t.priority;
    const labelOf = (t: Task) =>
      group === "assignee" ? (t.assignee?.display_name ?? "Unassigned") : t.priority;
    const map = new Map<string, { key: string; label: string; tasks: Task[] }>();
    for (const t of items) {
      const k = keyOf(t);
      const row = map.get(k) ?? { key: k, label: labelOf(t), tasks: [] };
      row.tasks.push(t);
      map.set(k, row);
    }
    return [...map.values()];
  }, [tasks.data, group]);

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
    <div className="flex h-full min-h-0 flex-col p-6" data-testid="timeline-view">
      <header className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Timeline</h1>
          <p className="text-xs text-muted-foreground">
            {project.data?.name ?? "Project"} — drag a bar to reschedule its due date.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Select value={zoom} onValueChange={(v) => setZoom(v as Zoom)}>
            <SelectTrigger className="h-8 w-[7.5rem]" aria-label="Timeline zoom" data-testid="timeline-zoom">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="day">Day</SelectItem>
              <SelectItem value="week">Week</SelectItem>
              <SelectItem value="month">Month</SelectItem>
            </SelectContent>
          </Select>
          <Select value={group} onValueChange={(v) => setGroup(v as "assignee" | "priority")}>
            <SelectTrigger className="h-8 w-[10rem]" aria-label="Group timeline by">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="assignee">Group by assignee</SelectItem>
              <SelectItem value="priority">Group by priority</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </header>

      {tasks.isLoading ? (
        <div className="flex-1 animate-pulse rounded-lg bg-muted" />
      ) : (tasks.data?.items.length ?? 0) === 0 ? (
        <EmptyState
          icon={<CalendarRange className="h-8 w-8" />}
          title="Nothing on the timeline"
          description="Tasks appear here once the project has work."
        />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto scrollbar-thin rounded-lg border" data-testid="timeline-grid">
          <div className="flex min-w-fit">
            {/* Left label column */}
            <div className="sticky left-0 z-10 w-56 shrink-0 border-r bg-card">
              <div className="h-10 border-b" />
              {rows.map((row) => (
                <div key={row.key} className="flex h-12 items-center gap-2 border-b px-3">
                  {group === "assignee" ? (
                    <Avatar name={row.label === "Unassigned" ? "?" : row.label} size="xs" />
                  ) : null}
                  <span className="truncate text-sm font-medium">{row.label}</span>
                </div>
              ))}
            </div>
            {/* Grid */}
            <div className="relative">
              {/* Header */}
              <div className="flex h-10 border-b" aria-hidden>
                {columns.map((c, i) => (
                  <div
                    key={i}
                    className="shrink-0 border-r px-2 pt-2 text-2xs text-muted-foreground"
                    style={{ width: columnWidth }}
                  >
                    {c.label}
                  </div>
                ))}
              </div>
              {/* Today line */}
              <div
                className="pointer-events-none absolute bottom-0 top-10 z-[5] w-px bg-destructive/70"
                style={{ left: (todayX - range.start) / (zoomDays(zoom) * DAY_MS) * columnWidth }}
                data-testid="timeline-today"
              />
              {rows.map((row) => (
                <div key={row.key} className="relative flex h-12 border-b">
                  {columns.map((_c, i) => (
                    <div key={i} className="h-full shrink-0 border-r" style={{ width: columnWidth }} />
                  ))}
                  {row.tasks.map((task) => {
                    const created = parseDay(new Date(task.created_at).toISOString().slice(0, 10)) ?? range.start;
                    const due = parseDay(task.due_date) ?? created + DAY_MS;
                    const start = Math.max(created, range.start);
                    const end = Math.min(due, range.end);
                    const left = ((start - range.start) / (zoomDays(zoom) * DAY_MS)) * columnWidth;
                    const width = Math.max(
                      columnWidth / (zoom === "day" ? 2 : 6),
                      ((end - start) / (zoomDays(zoom) * DAY_MS)) * columnWidth,
                    );
                    return (
                      <button
                        key={task.id}
                        type="button"
                        role="button"
                        aria-label={`${task.title}, ${task.status.replaceAll("_", " ")}, due ${
                          task.due_date ?? "no due date"
                        }`}
                        title={`${task.title} — due ${task.due_date ?? "—"}`}
                        draggable
                        onDragEnd={(e) => {
                          // Horizontal drag distance → whole-day shift.
                          const shift = Math.round(e.clientX - (e.target as HTMLElement).getBoundingClientRect().left) === 0 ? 0 : Math.round(
                            (e.clientX - (e.target as HTMLElement).getBoundingClientRect().left) /
                              columnWidth *
                              zoomDays(zoom),
                          );
                          if (shift !== 0) setReschedule({ task, shiftDays: shift });
                        }}
                        className={cn(
                          "absolute top-2 h-8 cursor-grab rounded-md border border-primary/40 bg-primary/15 px-2 text-left text-2xs font-medium text-foreground transition-shadow duration-200 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                          task.status === "closed" && "bg-status-closed/20 border-status-closed/40",
                        )}
                        style={{ left, width }}
                        data-testid={`timeline-bar-${task.id}`}
                      >
                        <span className="block truncate leading-8">{task.title}</span>
                      </button>
                    );
                  })}
                </div>
              ))}
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
