import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ListTodo, Search } from "lucide-react";
import { api } from "@/api/client";
import { useMyTasks } from "@/api/hooks";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { EmptyState, ErrorState } from "@/components/app/EmptyState";
import { TaskDetailPanel } from "@/components/app/TaskDetailPanel";
import { TaskTable } from "@/components/app/TaskTable";
import { PRIORITIES, PRIORITY_LABELS, TASK_STATUSES, STATUS_LABELS } from "@/lib/utils";

export function MyTasksPage() {
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const [priorityFilter, setPriorityFilter] = useState("all");
  const [page, setPage] = useState(1);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);

  const filters = useMemo(
    () => ({
      q: search || undefined,
      status: statusFilter !== "all" ? statusFilter : undefined,
      priority: priorityFilter !== "all" ? priorityFilter : undefined,
      sort: "due_date",
      order: "asc",
      page,
      page_size: 25,
    }),
    [search, statusFilter, priorityFilter, page],
  );

  const tasks = useMyTasks(filters);
  const detail = useQuery({
    queryKey: ["task", selectedTaskId],
    queryFn: () => api.getTask(selectedTaskId!),
    enabled: Boolean(selectedTaskId),
  });

  const totalPages = tasks.data ? Math.max(1, Math.ceil(tasks.data.total / 25)) : 1;

  return (
    <div className="flex h-full min-h-0">
      <div className="min-w-0 flex-1 overflow-y-auto scrollbar-thin p-6">
        <header className="mb-4 flex flex-wrap items-end justify-between gap-3">
          <div>
            <h1 className="text-lg font-semibold">My Tasks</h1>
            <p className="text-xs text-muted-foreground">
              Everything assigned to you across all teams, due soonest first.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2" data-testid="my-tasks-filters">
            <div className="relative">
              <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={search}
                onChange={(e) => {
                  setSearch(e.target.value);
                  setPage(1);
                }}
                placeholder="Search…"
                className="w-48 pl-7"
                aria-label="Search my tasks"
              />
            </div>
            <Select
              value={statusFilter}
              onValueChange={(v) => {
                setStatusFilter(v);
                setPage(1);
              }}
            >
              <SelectTrigger className="w-auto min-w-[7.5rem]" aria-label="Filter by status">
                <SelectValue placeholder="Status" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All statuses</SelectItem>
                {TASK_STATUSES.map((s) => (
                  <SelectItem key={s} value={s}>
                    {STATUS_LABELS[s]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select
              value={priorityFilter}
              onValueChange={(v) => {
                setPriorityFilter(v);
                setPage(1);
              }}
            >
              <SelectTrigger className="w-auto min-w-[7.5rem]" aria-label="Filter by priority">
                <SelectValue placeholder="Priority" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All priorities</SelectItem>
                {PRIORITIES.map((p) => (
                  <SelectItem key={p} value={p}>
                    {PRIORITY_LABELS[p]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </header>

        {tasks.isError ? (
          <ErrorState title="Could not load your tasks" onRetry={() => void tasks.refetch()} />
        ) : !tasks.isLoading && (tasks.data?.items.length ?? 0) === 0 ? (
          <EmptyState
            icon={<ListTodo className="h-8 w-8" />}
            title="You're all caught up"
            description="No tasks are assigned to you. Pick work from a project board or ask your team."
          />
        ) : (
          <TaskTable
            tasks={tasks.data?.items ?? []}
            total={tasks.data?.total}
            selectedId={selectedTaskId ?? undefined}
            onSelect={(task) => setSelectedTaskId(task.id)}
            isLoading={tasks.isLoading}
            renderContext={(task) => {
              const row = task as typeof task & { team_name?: string; project_name?: string };
              return [row.team_name, row.project_name].filter(Boolean).join(" · ");
            }}
          />
        )}

        {totalPages > 1 ? (
          <nav className="mt-3 flex items-center justify-end gap-2" aria-label="Pagination">
            <Button variant="outline" size="sm" disabled={page <= 1} onClick={() => setPage((p) => p - 1)}>
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
      </div>

      {selectedTaskId ? (
        <aside className="hidden w-[380px] shrink-0 border-l lg:block">
          <TaskDetailPanel task={detail.data} onClose={() => setSelectedTaskId(null)} />
        </aside>
      ) : null}
    </div>
  );
}
