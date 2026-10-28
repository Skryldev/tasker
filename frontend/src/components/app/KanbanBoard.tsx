import { useState } from "react";
import {
  DndContext,
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
import { Plus } from "lucide-react";
import type { BoardColumn, Task } from "@/api/client";
import { api } from "@/api/client";
import { useToast } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/badge";
import { Avatar } from "@/components/ui/avatar";
import { PriorityBadge } from "./TaskBadges";
import { cn, STATUS_LABELS, type TaskStatus } from "@/lib/utils";

function statusToClass(status: string): string {
  switch (status) {
    case "todo":
      return "bg-status-todo";
    case "in_progress":
      return "bg-status-in-progress";
    case "in_review":
      return "bg-status-in-review";
    case "done":
      return "bg-status-done";
    default:
      return "bg-status-backlog";
  }
}

function TaskCard({
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
      aria-label={`Task: ${task.title}`}
      onKeyDown={(e) => {
        if (overlay) return;
        if (e.key === "Enter") onOpen?.(task);
      }}
      onClick={() => onOpen?.(task)}
      className={cn(
        "cursor-grab rounded-lg border bg-card p-3 shadow-sm transition-shadow hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        overlay && "rotate-1 shadow-lg",
        isDragging && "opacity-40",
      )}
      data-testid={`kanban-card-${task.id}`}
    >
      <p className="text-sm font-medium leading-snug">{task.title}</p>
      <div className="mt-2 flex items-center justify-between gap-2">
        <PriorityBadge priority={task.priority} />
        {task.assignee ? (
          <Avatar name={task.assignee.display_name} size="xs" />
        ) : (
          <span className="text-2xs text-muted-foreground">Unassigned</span>
        )}
      </div>
    </div>
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
          isOver && "rounded-lg bg-primary/5 ring-1 ring-primary/30",
        )}
      >
        {column.items.map((task) => (
          <TaskCard key={task.id} task={task} onOpen={onOpen} />
        ))}
      </div>
    </section>
  );
}

export function KanbanBoard({
  projectId,
  columns,
  isLoading,
  onOpenTask,
  onCreateTask,
}: {
  projectId: string;
  columns: BoardColumn[] | undefined;
  isLoading: boolean;
  onOpenTask: (task: Task) => void;
  onCreateTask: (status: string) => void;
}) {
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));
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
      await api.updateTask(task.id, { status: targetStatus as Parameters<typeof api.updateTask>[1]["status"] });
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

  if (isLoading) {
    return (
      <div className="flex gap-3 p-4">
        {[0, 1, 2, 3, 4].map((i) => (
          <div key={i} className="flex w-[270px] shrink-0 flex-col gap-2">
            <Skeleton className="h-5 w-24" />
            <Skeleton className="h-20 w-full" />
            <Skeleton className="h-20 w-full" />
          </div>
        ))}
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
