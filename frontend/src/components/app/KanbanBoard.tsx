import { memo, useMemo, useState } from "react";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  useDroppable,
  useDraggable,
  DragOverlay,
  type DragStartEvent,
  type DragEndEvent,
} from "@dnd-kit/core";
import { useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Flag, ListChecks, Plus } from "lucide-react";
import type { BoardColumn, Task } from "@/api/client";
import { api } from "@/api/client";
import { useToast } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import { Badge, Skeleton } from "@/components/ui/badge";
import { Avatar } from "@/components/ui/avatar";
import { LabelIcon, PriorityBadge } from "./TaskBadges";
import { cn, PRIORITY_LABELS, STATUS_LABELS, type TaskStatus } from "@/lib/utils";

function statusToClass(status: string): string {
  switch (status) {
    case "todo":
      return "bg-status-todo";
    case "in_progress":
      return "bg-status-in-progress";
    case "resolved":
      return "bg-status-resolved";
    case "in_review":
      return "bg-status-in-review";
    case "closed":
      return "bg-status-closed";
    default:
      return "bg-status-backlog";
  }
}

/** Memoized: the board re-renders on drag state changes; cards do not need to. */
const TaskCard = memo(function TaskCard({
  task,
  onOpen,
  overlay,
}: {
  task: Task;
  onOpen?: (task: Task) => void;
  overlay?: boolean;
}) {
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: task.id,
    data: { task },
    disabled: overlay,
  });
  return (
    <div
      ref={setNodeRef}
      {...attributes}
      {...listeners}
      role="button"
      tabIndex={0}
      aria-roledescription="Draggable task"
      // Label in Name (WCAG 2.5.3): the accessible name must contain the
      // card's full visible text (title, priority label, unassigned marker)
      // as a continuous string.
      aria-label={
        task.assignee
          ? `${task.title} ${PRIORITY_LABELS[task.priority] ?? task.priority} priority`
          : `${task.title} ${PRIORITY_LABELS[task.priority] ?? task.priority} priority unassigned`
      }
      onKeyDown={(e) => {
        if (overlay) return;
        if (e.key === "Enter") onOpen?.(task);
      }}
      onClick={() => onOpen?.(task)}
      className={cn(
        "cursor-grab rounded-lg border bg-card p-3 shadow-sm transition-[box-shadow,transform] duration-150 ease-out hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        overlay && "scale-[1.02] shadow-lg",
        isDragging && "opacity-40",
      )}
      data-testid={`kanban-card-${task.id}`}
    >
      <p className="line-clamp-2 text-sm font-medium leading-snug">{task.title}</p>
      {/* Whitespace between the visible spans so textContent matches the
          accessible name (Label-in-Name substring check). Renders as nothing. */}
      {" "}
      <div className="mt-2 flex items-center gap-1.5">
        <PriorityBadge priority={task.priority} />
        {task.deadline ? (
          <span
            className="inline-flex items-center gap-0.5 rounded-full border border-destructive/40 bg-destructive/10 px-1.5 py-0.5 text-2xs font-medium text-destructive"
            title={`Deadline ${task.deadline}`}
            data-testid={`kanban-deadline-${task.id}`}
          >
            <Flag className="h-2.5 w-2.5" aria-hidden />
            {task.deadline.slice(5)}
          </span>
        ) : null}
        {task.labels.slice(0, 2).map((l) => (
          <span
            key={l.id}
            className="inline-flex max-w-[7rem] items-center gap-1 rounded-full border px-1.5 py-0.5 text-2xs"
            style={{ backgroundColor: `${l.color}1f`, borderColor: `${l.color}55` }}
          >
            {l.icon ? <LabelIcon icon={l.icon} /> : null}
            <span className="truncate">{l.name}</span>
          </span>
        ))}
        {task.labels.length > 2 ? (
          <span className="text-2xs text-muted-foreground">+{task.labels.length - 2}</span>
        ) : null}
        {progressPill(task)}
      </div>
      <div className="mt-2 flex items-center justify-between gap-2">
        {task.assignee ? (
          <Avatar name={task.assignee.display_name} size="xs" />
        ) : (
          <span className="text-2xs text-muted-foreground">Unassigned</span>
        )}
      </div>
    </div>
  );
});

/** WS4: a compact n/total pill when the board task carries subtasks. */
function progressPill(task: Task) {
  const subs = task.subtasks;
  if (!subs || subs.length === 0) return null;
  const closed = subs.filter((s) => s.status === "closed").length;
  return (
    <span
      className="ml-auto inline-flex items-center gap-1 rounded-full bg-muted px-1.5 py-0.5 text-2xs tabular-nums text-muted-foreground"
      title={`${closed} of ${subs.length} subtasks closed`}
    >
      <ListChecks className="h-2.5 w-2.5" aria-hidden />
      {closed}/{subs.length}
    </span>
  );
}

function KanbanColumn({
  column,
  onOpen,
  onAdd,
}: {
  column: BoardColumn;
  onOpen: (task: Task) => void;
  onAdd: (status: string) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: column.status });
  const status: TaskStatus = (column.status ?? "backlog") as TaskStatus;
  return (
    <section
      aria-label={`${STATUS_LABELS[status] ?? column.status} column`}
      className="flex w-[270px] shrink-0 flex-col rounded-lg bg-muted/40"
    >
      <header className="flex items-center gap-2 px-3 pb-1 pt-3">
        <span className={cn("h-2 w-2 rounded-full", statusToClass(column.status))} />
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {STATUS_LABELS[status]}
        </h3>
        <span className="text-2xs text-muted-foreground">
          {column.items.length === column.total
            ? column.total
            : `${column.items.length}/${column.total}`}
        </span>
        <Button
          variant="ghost"
          size="icon-sm"
          className="ml-auto"
          aria-label={`Add task to ${STATUS_LABELS[status] ?? column.status}`}
          onClick={() => onAdd(column.status)}
        >
          <Plus />
        </Button>
      </header>
        <div
          ref={setNodeRef}
          data-testid={`kanban-column-${column.status}`}
          className={cn(
            "flex min-h-[80px] flex-1 flex-col gap-2 overflow-y-auto scrollbar-thin p-2 transition-colors",
            // WS7: the drop target flashes a dashed green outline.
            isOver && "rounded-lg border-2 border-dashed border-success bg-success/5",
          )}
        >
        {column.items.map((task) => (
          <TaskCard key={task.id} task={task} onOpen={onOpen} />
        ))}
      </div>
    </section>
  );
}

export type SwimlaneMode = "none" | "assignee" | "priority" | "label";

export function KanbanBoard({
  projectId,
  columns,
  isLoading,
  onOpenTask,
  onCreateTask,
  swimlane = "none",
  onSwimlaneDrag,
}: {
  projectId: string;
  columns: BoardColumn[] | undefined;
  isLoading: boolean;
  onOpenTask: (task: Task) => void;
  onCreateTask: (status: string) => void;
  /** Grouping mode (Phase 11). `none` renders the classic single-lane board. */
  swimlane?: SwimlaneMode;
  /** Drag between assignee/priority/label swimlanes: applies the field change. */
  onSwimlaneDrag?: (task: Task, field: "assignee" | "priority" | "label", value: string | null) => void;
}) {
  // Pointer for mouse/touch; keyboard so cards can be moved with Space +
  // arrows (dnd-kit announces instructions via the card's aria attributes).
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor),
  );
  const { push } = useToast();
  const qc = useQueryClient();
  const [activeTask, setActiveTask] = useState<Task | null>(null);

  const onDragStart = (event: DragStartEvent) => {
    setActiveTask((event.active.data.current?.task as Task) ?? null);
  };

  const onDragEnd = async (event: DragEndEvent) => {
    setActiveTask(null);
    const overId = event.over?.id;
    const task = event.active.data.current?.task as Task | undefined;
    if (!overId || !task) return;
    const targetStatus = String(overId);
    if (targetStatus === task.status) return;

    // Optimistic move with rollback on failure.
    const boardKey = ["board", projectId];
    const previous = qc.getQueryData(boardKey);
    qc.setQueryData(boardKey, (old: { project_id: string; columns: BoardColumn[] } | undefined) => {
      if (!old) return old;
      return {
        ...old,
        columns: old.columns.map((c) => {
          const items = c.items.filter((t) => t.id !== task.id);
          if (c.status === targetStatus) {
            return { ...c, items: [{ ...task, status: targetStatus }, ...items], total: c.total + 1 };
          }
          if (c.status === task.status) {
            return { ...c, items, total: Math.max(0, c.total - 1) };
          }
          return { ...c, items };
        }),
      };
    });
    try {
      // Column moves go through the lifecycle transition endpoint; the backend
      // enforces the state machine and role rules and rejects illegal moves
      // (the optimistic card position is rolled back below).
      await api.transitionTask(task.id, targetStatus);
      void qc.invalidateQueries({ queryKey: ["board", projectId] });
      void qc.invalidateQueries({ queryKey: ["tasks"] });
      void qc.invalidateQueries({ queryKey: ["my-tasks"] });
      void qc.invalidateQueries({ queryKey: ["activity", task.id] });
    } catch (error) {
      qc.setQueryData(boardKey, previous);
      push(
        "error",
        "Could not move task",
        error instanceof Error ? error.message : "The board was restored to its previous state.",
      );
    }
  };

  // Swimlane grouping (Phase 11): partition the flat board into lanes by
  // assignee / priority / label. Cards not matching a named lane fall into
  // the trailing "Unassigned" lane. Lanes mount lazily: columns render on
  // first visibility (IntersectionObserver with a pre-scroll margin).
  const lanes = useMemo(() => {
    if (swimlane === "none") return [];
    const map = new Map<
      string,
      { key: string; label: string; avatar?: string; tone?: string; columns: BoardColumn[] }
    >();
    for (const column of columns ?? []) {
      for (const task of column.items) {
        let key = "none";
        let label = "Unassigned";
        let avatar: string | undefined;
        let tone: string | undefined;
        if (swimlane === "assignee") {
          key = task.assignee?.id ?? "none";
          label = task.assignee?.display_name ?? "Unassigned";
          avatar = label;
        } else if (swimlane === "priority") {
          key = task.priority;
          label = PRIORITY_LABELS[task.priority as keyof typeof PRIORITY_LABELS] ?? task.priority;
          tone = task.priority;
        } else if (swimlane === "label") {
          key = task.labels[0]?.id ?? "none";
          label = task.labels[0]?.name ?? "No label";
          tone = task.labels[0]?.color;
        }
        const lane =
          map.get(key) ?? { key, label, avatar, tone, columns: [] as BoardColumn[] };
        let laneColumn = lane.columns.find((c) => c.status === column.status);
        if (!laneColumn) {
          laneColumn = { status: column.status, items: [], total: 0 };
          lane.columns.push(laneColumn);
        }
        laneColumn.items = [...laneColumn.items.filter((t) => t.id !== task.id), task];
        laneColumn.total += 1;
        map.set(key, lane);
      }
    }
    const list = [...map.values()];
    const unassigned = list.find((l) => l.key === "none");
    return [...list.filter((l) => l.key !== "none"), ...(unassigned ? [unassigned] : [])];
  }, [columns, swimlane]);

  const [collapsedLanes, setCollapsedLanes] = useState<Set<string>>(new Set());
  const [showEmpty, setShowEmpty] = useState(false);
  const [mountedLanes, setMountedLanes] = useState<Set<string>>(new Set());

  const laneRef = (key: string) => (el: HTMLElement | null) => {
    if (el && !mountedLanes.has(key)) {
      const observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) {
              setMountedLanes((prev) => new Set(prev).add(key));
              observer.disconnect();
            }
          }
        },
        { rootMargin: "200px" },
      );
      observer.observe(el);
    }
  };

  if (isLoading) {
    return (
      <div className="flex gap-3 p-4">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="flex w-[270px] shrink-0 flex-col gap-2">
            <Skeleton className="h-5 w-24" />
            <Skeleton className="h-20 w-full" />
            <Skeleton className="h-20 w-full" />
          </div>
        ))}
      </div>
    );
  }


  // Swimlane drag-end: moving a card across lanes changes the grouping field
  // (assignee / priority / label) through the parent's mutation with an undo
  // toast; within-lane moves keep the status-transition semantics.
  const onSwimlaneDragEnd = (event: DragEndEvent) => {
    const task = event.active.data.current?.task as Task | undefined;
    if (!task || !event.over) return;
    const overId = String(event.over.id);
    if (swimlane === "assignee" && overId.startsWith("member:")) {
      const value = overId.slice(7);
      if ((task.assignee?.id ?? "none") !== value) {
        onSwimlaneDrag?.(task, "assignee", value === "none" ? null : value);
      }
      return;
    }
    if (swimlane === "priority" && overId.startsWith("priority:")) {
      const value = overId.slice(9);
      if (task.priority !== value) {
        onSwimlaneDrag?.(task, "priority", value);
      }
      return;
    }
    if (swimlane === "label" && overId.startsWith("label:")) {
      const value = overId.slice(6);
      const has = task.labels.some((l) => l.id === value);
      if (!has) {
        onSwimlaneDrag?.(task, "label", value);
      }
      return;
    }
    // Fall through to status-transition semantics for drops on columns.
    onDragEnd(event);
  };

  if (swimlane !== "none") {
    const orderedColumns = columns ?? [];
    const visibleLanes = lanes.filter((lane) => {
      if (lane.key === "none") return showEmpty || lane.columns.some((c) => c.items.length > 0);
      return true;
    });
    return (
      <div className="flex h-full flex-col" data-testid="kanban-swimlanes">
        <div className="flex items-center justify-end gap-2 px-4 pt-2">
          <label className="flex items-center gap-1.5 text-2xs text-muted-foreground">
            <input
              type="checkbox"
              checked={showEmpty}
              onChange={(e) => setShowEmpty(e.target.checked)}
              data-testid="swimlane-show-empty"
            />
            Show empty lanes
          </label>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto scrollbar-thin">
          {visibleLanes.map((lane) => {
            const collapsed = collapsedLanes.has(lane.key);
            const mounted = mountedLanes.has(lane.key);
            return (
              <section
                key={lane.key}
                ref={laneRef(lane.key)}
                role="group"
                aria-labelledby={"swimlane-" + lane.key}
                className="border-t first:border-t-0"
                data-testid={"swimlane-" + lane.key}
              >
                <header className="sticky top-0 z-[5] flex items-center gap-2 border-b bg-card/95 px-4 py-2 backdrop-blur">
                  <button
                    type="button"
                    aria-expanded={!collapsed}
                    aria-controls={"swimlane-body-" + lane.key}
                    onClick={() =>
                      setCollapsedLanes((prev) => {
                        const next = new Set(prev);
                        if (next.has(lane.key)) next.delete(lane.key);
                        else next.add(lane.key);
                        return next;
                      })
                    }
                    className="flex items-center gap-2 rounded text-sm font-semibold hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    data-testid={"swimlane-toggle-" + lane.key}
                  >
                    <ChevronDown
                      className={cn(
                        "h-4 w-4 transition-transform duration-200",
                        collapsed && "-rotate-90",
                      )}
                      aria-hidden
                    />
                    {lane.avatar ? <Avatar name={lane.avatar} size="xs" /> : null}
                    {lane.tone && lane.tone.startsWith("#") ? (
                      <span
                        className="h-2 w-2 rounded-full"
                        style={{ backgroundColor: lane.tone }}
                        aria-hidden
                      />
                    ) : null}
                    <span id={"swimlane-" + lane.key}>{lane.label}</span>
                  </button>
                  <Badge variant="muted">
                    {lane.columns.reduce((sum, c) => sum + c.items.length, 0)}
                  </Badge>
                </header>
                <div
                  id={"swimlane-body-" + lane.key}
                  className={cn("transition-opacity duration-200", collapsed ? "hidden" : "block")}
                >
                  {mounted || !collapsed ? (
                    <DndContext
                      sensors={sensors}
                      onDragStart={onDragStart}
                      onDragEnd={onSwimlaneDragEnd}
                    >
                      <div
                        className="flex h-full gap-3 overflow-x-auto scrollbar-thin p-4"
                        data-testid={"kanban-board-" + lane.key}
                      >
                        {orderedColumns.map((statusColumn) => {
                          const laneColumn =
                            lane.columns.find((c) => c.status === statusColumn.status) ??
                            ({ status: statusColumn.status, items: [], total: 0 } as BoardColumn);
                          return (
                            <KanbanColumn
                              key={statusColumn.status}
                              column={laneColumn}
                              onOpen={onOpenTask}
                              onAdd={onCreateTask}
                            />
                          );
                        })}
                      </div>
                      <DragOverlay>
                        {activeTask ? <TaskCard task={activeTask} overlay /> : null}
                      </DragOverlay>
                    </DndContext>
                  ) : null}
                </div>
              </section>
            );
          })}
          {visibleLanes.length === 0 ? (
            <p className="p-8 text-center text-sm text-muted-foreground">
              No tasks in this swimlane mode.
            </p>
          ) : null}
        </div>
      </div>
    );
  }

  return (
    <DndContext sensors={sensors} onDragStart={onDragStart} onDragEnd={onDragEnd}>
      <div className="flex h-full gap-3 overflow-x-auto scrollbar-thin p-4" data-testid="kanban-board">
        {(columns ?? []).map((column) => (
          <KanbanColumn key={column.status} column={column} onOpen={onOpenTask} onAdd={onCreateTask} />
        ))}
      </div>
      <DragOverlay>{activeTask ? <TaskCard task={activeTask} overlay /> : null}</DragOverlay>
    </DndContext>
  );
}
