import { useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { KanbanSquare, ListFilter, ListTodo, Plus, Search } from "lucide-react";
import { api } from "@/api/client";
import { TemplateManager } from "@/components/app/TemplateManager";
import { useLabels, useMembers, useBoard, useTasks, useProject } from "@/api/hooks";
import type { Task } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { KanbanBoard } from "@/components/app/KanbanBoard";
import { TaskTable } from "@/components/app/TaskTable";
import { TaskDetailPanel } from "@/components/app/TaskDetailPanel";
import { TaskDialog } from "@/components/app/TaskDialog";
import { EmptyState, ErrorState } from "@/components/app/EmptyState";
import { PRIORITIES, PRIORITY_LABELS, TASK_STATUSES, STATUS_LABELS } from "@/lib/utils";
import { cn } from "@/lib/utils";

type View = "list" | "board";

export function ProjectPage() {
  const { projectId } = useParams<{ projectId: string }>();
  const [params, setParams] = useSearchParams();
  const view: View = params.get("view") === "board" ? "board" : "list";

  const project = useProject(projectId);
  const teamId = project.data?.team_id;

  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const [priorityFilter, setPriorityFilter] = useState("all");
  const [assigneeFilter, setAssigneeFilter] = useState("all");
  const [labelFilter, setLabelFilter] = useState("all");
  const [sort, setSort] = useState("created_at");
  const [order, setOrder] = useState("desc");
  const [page, setPage] = useState(1);
  const [selectedTask, setSelectedTask] = useState<Task | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);

  const members = useMembers(teamId);
  const labels = useLabels(teamId);

  const filters = useMemo(
    () => ({
      q: search || undefined,
      status: statusFilter !== "all" ? statusFilter : undefined,
      priority: priorityFilter !== "all" ? priorityFilter : undefined,
      assignee_id:
        assigneeFilter !== "all" ? (assigneeFilter === "none" ? "none" : assigneeFilter) : undefined,
      label_id: labelFilter !== "all" ? labelFilter : undefined,
      sort,
      order,
      page,
      page_size: 25,
    }),
    [search, statusFilter, priorityFilter, assigneeFilter, labelFilter, sort, order, page],
  );

  const tasks = useTasks(view === "list" ? projectId : undefined, filters);
  const board = useBoard(projectId);

  const isGuestProject = project.data?.is_guest === true;

  if (project.isLoading) {
    return (
      <div className="p-6">
        <div className="h-6 w-48 animate-pulse rounded bg-muted" />
      </div>
    );
  }
  if (project.isError) {
    return (
      <div className="p-6">
        <ErrorState
          title="Project unavailable"
          detail="It may have been archived by an administrator or you may lack access."
          onRetry={() => void project.refetch()}
        />
      </div>
    );
  }

  const setView = (v: View) => {
    const next = new URLSearchParams(params);
    next.set("view", v);
    setParams(next, { replace: true });
  };

  const totalPages = tasks.data ? Math.max(1, Math.ceil(tasks.data.total / 25)) : 1;

  const openCreate = (_status?: string) => {
    setDialogOpen(true);
  };

  const openTaskDetail = async (task: Task) => {
    // Fetch fresh detail (with subtasks) when selecting.
    setSelectedTask(task);
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="border-b px-6 pb-3 pt-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-3">
            <h1 className="text-lg font-semibold">{project.data!.name}</h1>
            {project.data!.status === "archived" ? (
              <span className="rounded-full bg-muted px-2 py-0.5 text-2xs text-muted-foreground">
                Archived
              </span>
            ) : null}
          </div>
          <div className="flex items-center gap-1 rounded-lg bg-muted p-0.5" role="tablist" aria-label="View switcher">
            {(
              [
                ["list", "List", <ListTodo key="l" />],
                ["board", "Board", <KanbanSquare key="k" />],
              ] as const
            ).map(([key, label, icon]) => (
              <button
                key={key}
                role="tab"
                aria-selected={view === key}
                onClick={() => setView(key)}
                className={cn(
                  "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium transition-colors",
                  view === key
                    ? "bg-background shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                data-testid={`view-${key}`}
              >
                {icon}
                {label}
              </button>
            ))}
          </div>
        </div>

        {view === "list" ? (
          <div className="mt-3 flex flex-wrap items-center gap-2" data-testid="task-filters">
            <div className="relative">
              <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={search}
                onChange={(e) => {
                  setSearch(e.target.value);
                  setPage(1);
                }}
                placeholder="Search tasks…"
                className="w-52 pl-7"
                aria-label="Search tasks"
              />
            </div>
            <FilterSelect
              label="Status"
              value={statusFilter}
              onChange={(v) => {
                setStatusFilter(v);
                setPage(1);
              }}
              allLabel="All statuses"
              options={TASK_STATUSES.map((s) => [s, STATUS_LABELS[s]])}
            />
            <FilterSelect
              label="Priority"
              value={priorityFilter}
              onChange={(v) => {
                setPriorityFilter(v);
                setPage(1);
              }}
              allLabel="All priorities"
              options={PRIORITIES.map((p) => [p, PRIORITY_LABELS[p]])}
            />
            <FilterSelect
              label="Assignee"
              value={assigneeFilter}
              onChange={(v) => {
                setAssigneeFilter(v);
                setPage(1);
              }}
              allLabel="Anyone"
              options={[
                ["none", "Unassigned"],
                ...(members.data?.items.map((m) => [m.user.id, m.user.display_name] as const) ?? []),
              ]}
            />
            {labels.data && labels.data.items.length > 0 ? (
              <FilterSelect
                label="Label"
                value={labelFilter}
                onChange={(v) => {
                  setLabelFilter(v);
                  setPage(1);
                }}
                allLabel="All labels"
                options={labels.data.items.map((l) => [l.id, l.name])}
              />
            ) : null}
            <FilterSelect
              label="Sort"
              value={`${sort}:${order}`}
              onChange={(v) => {
                const [s, o] = v.split(":");
                setSort(s ?? "created_at");
                setOrder(o ?? "desc");
              }}
              allLabel={undefined}
              options={[
                ["created_at:desc", "Newest"],
                ["created_at:asc", "Oldest"],
                ["due_date:asc", "Due soonest"],
                ["priority:asc", "Priority ↑"],
                ["priority:desc", "Priority ↓"],
                ["title:asc", "Title A–Z"],
                ["updated_at:desc", "Recently updated"],
              ]}
            />
            <div className="ml-auto flex items-center gap-2">
              <span className="text-2xs text-muted-foreground">
                {tasks.data ? `${tasks.data.total} task${tasks.data.total === 1 ? "" : "s"}` : ""}
              </span>
              {isGuestProject ? null : (
                <Button size="sm" onClick={() => openCreate()} data-testid="create-task">
                  <Plus /> New task
                </Button>
              )}
            </div>
          </div>
        ) : (
          <div className="mt-3 flex justify-end">
            {isGuestProject ? null : (
              <Button size="sm" onClick={() => openCreate()} data-testid="create-task-board">
                <Plus /> New task
              </Button>
            )}
          </div>
        )}
      </header>

      {isGuestProject ? (
        <div
          className="border-b bg-primary/5 px-6 py-2 text-xs text-muted-foreground"
          data-testid="guest-banner"
        >
          You have read-only guest access to this project.
        </div>
      ) : null}

      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1 overflow-y-auto scrollbar-thin p-4">
          {view === "list" ? (
            tasks.isError ? (
              <ErrorState title="Could not load tasks" onRetry={() => void tasks.refetch()} />
            ) : (tasks.data?.items.length ?? 0) === 0 && !tasks.isLoading ? (
              <EmptyState
                icon={<ListFilter className="h-8 w-8" />}
                title="No tasks match"
                description="Adjust the filters, or create the first task in this project."
                action={
                  <Button onClick={() => openCreate()}>
                    <Plus /> New task
                  </Button>
                }
              />
            ) : (
              <>
                <TaskTable
                  tasks={tasks.data?.items ?? []}
                  total={tasks.data?.total}
                  selectedId={selectedTask?.id}
                  onSelect={(task) => void openTaskDetail(task)}
                  isLoading={tasks.isLoading}
                />
                {totalPages > 1 ? (
                  <nav className="mt-3 flex items-center justify-end gap-2" aria-label="Pagination">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={page <= 1}
                      onClick={() => setPage((p) => p - 1)}
                    >
                      Previous
                    </Button>
                    <span className="text-2xs text-muted-foreground">
                      Page {page} of {totalPages}
                    </span>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={page >= totalPages}
                      onClick={() => setPage((p) => p + 1)}
                    >
                      Next
                    </Button>
                  </nav>
                ) : null}
              </>
            )
          ) : (
            <KanbanBoard
              projectId={projectId!}
              columns={board.data?.columns}
              isLoading={board.isLoading}
              onOpenTask={(task) => void openTaskDetail(task)}
              onCreateTask={(status) => openCreate(status)}
            />
          )}
          {isGuestProject ? null : (
            <TemplateManager projectId={projectId!} />
          )}
        </div>

        {selectedTask ? (
          <aside className="hidden w-[380px] shrink-0 border-l lg:block" data-testid="task-panel">
            <TaskDetailLoader taskId={selectedTask.id} onClose={() => setSelectedTask(null)} />
          </aside>
        ) : null}
      </div>

      {dialogOpen && teamId ? (
        <TaskDialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          teamId={teamId}
          projectId={projectId!}
        />
      ) : null}
    </div>
  );
}

/** Load the full task (with subtasks) for the detail panel. */
function TaskDetailLoader({ taskId, onClose }: { taskId: string; onClose: () => void }) {
  const query = useQuery({
    queryKey: ["task", taskId],
    queryFn: () => api.getTask(taskId),
  });
  return <TaskDetailPanel task={query.data} onClose={onClose} />;
}

function FilterSelect({
  label,
  value,
  onChange,
  options,
  allLabel,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: readonly (readonly [string, string])[];
  allLabel?: string;
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className="w-auto min-w-[7.5rem]" aria-label={`Filter by ${label}`}>
        <SelectValue placeholder={label} />
      </SelectTrigger>
      <SelectContent>
        {allLabel ? <SelectItem value="all">{allLabel}</SelectItem> : null}
        {options.map(([value_, label_]) => (
          <SelectItem key={value_} value={value_}>
            {label_}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
