import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { Link2, ListTree, Plus, Trash2, Users } from "lucide-react";
import { api, ApiError, type Task } from "@/api/client";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast";
import { cn, STATUS_LABELS, type TaskStatus } from "@/lib/utils";

interface DepRef {
  task_id: string;
  title: string;
  status: string;
  due_date?: string | null;
}

/** Dependency panel for the task detail view: blockers above, blocked below.
 *  Team members can add/remove edges; guests see a read-only list. */
export function DependenciesPanel({ task }: { task: Task }) {
  const qc = useQueryClient();
  const { push } = useToast();
  const [addOpen, setAddOpen] = useState(false);
  const [candidate, setCandidate] = useState<string>("");

  const deps = useQuery({
    queryKey: ["dependencies", task.id],
    queryFn: () => api.listDependencies(task.id),
  });

  const candidates = useQuery({
    queryKey: ["tasks", task.project_id, "all-for-deps"],
    queryFn: () => api.listTasks(task.project_id, { page_size: 100 }),
    enabled: addOpen,
  });

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["dependencies", task.id] });
  };

  const add = async () => {
    if (!candidate || candidate === task.id) return;
    try {
      await api.createDependency(task.id, candidate);
      setAddOpen(false);
      setCandidate("");
      refresh();
    } catch (error) {
      push(
        "error",
        "Could not add dependency",
        error instanceof ApiError ? error.message : "Check the task selection.",
      );
    }
  };

  const remove = async (predecessorId: string) => {
    try {
      await api.removeDependency(task.id, predecessorId);
      refresh();
    } catch (error) {
      push("error", "Could not remove dependency", error instanceof Error ? error.message : undefined);
    }
  };

  const refRow = (ref: DepRef, onRemove?: () => void) => (
    <div
      key={ref.task_id}
      className="flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-sm"
      data-testid={`dep-${ref.task_id}`}
    >
      <span
        className={cn(
          "h-2 w-2 shrink-0 rounded-full",
          ref.status === "done" ? "bg-status-done" : "bg-status-in-progress",
        )}
        aria-label={ref.status === "done" ? "done" : "in progress"}
      />
      <span className={cn("truncate", ref.status === "done" && "text-muted-foreground line-through")}>
        {ref.title}
      </span>
      <span className="ml-auto shrink-0 text-2xs text-muted-foreground">
        {STATUS_LABELS[ref.status as TaskStatus] ?? ref.status}
      </span>
      {onRemove ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label="Remove dependency"
          className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
        >
          <Trash2 className="h-3 w-3" />
        </button>
      ) : null}
    </div>
  );

  const blockers = deps.data?.blockers ?? [];
  const blocked = deps.data?.blocked ?? [];
  const canManage = deps.data?.can_manage ?? false;
  const openBlockers = blockers.filter((b) => b.status !== "done").length;

  if (deps.isLoading) {
    return <div className="h-16 animate-pulse rounded bg-muted" />;
  }

  return (
    <section aria-label="Dependencies" className="mt-5" data-testid="dependencies-panel">
      <div className="mb-2 flex items-center justify-between">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold">
          <Link2 className="h-3.5 w-3.5" /> Dependencies
          {openBlockers > 0 ? (
            <Badge className="ml-1" variant="muted" data-testid="blocked-count">
              {openBlockers} open blocker{openBlockers === 1 ? "" : "s"}
            </Badge>
          ) : null}
        </h3>
        {canManage ? (
          <Button variant="ghost" size="sm" onClick={() => setAddOpen((v) => !v)} data-testid="add-dependency">
            <Plus /> Add
          </Button>
        ) : null}
      </div>

      {addOpen ? (
        <div className="mb-3 flex items-center gap-2" data-testid="dependency-picker">
          <Select value={candidate} onValueChange={setCandidate}>
            <SelectTrigger aria-label="Choose a predecessor task" className="flex-1">
              <SelectValue placeholder="Select a task that must finish first" />
            </SelectTrigger>
            <SelectContent>
              {(candidates.data?.items ?? [])
                .filter((t) => t.id !== task.id)
                .map((t) => (
                  <SelectItem key={t.id} value={t.id}>
                    {t.title}
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>
          <Button size="sm" onClick={() => void add()} disabled={!candidate} data-testid="dependency-submit">
            Link
          </Button>
        </div>
      ) : null}

      {blockers.length === 0 && blocked.length === 0 ? (
        <p className="text-xs text-muted-foreground">No dependencies.</p>
      ) : (
        <div className="flex flex-col gap-3">
          {blockers.length > 0 ? (
            <div>
              <p className="mb-1 text-2xs uppercase tracking-wide text-muted-foreground">
                Blocked by (must finish first)
              </p>
              <div className="flex flex-col gap-1">
                {blockers.map((b) => refRow(b, canManage ? () => void remove(b.task_id) : undefined))}
              </div>
            </div>
          ) : null}
          {blocked.length > 0 ? (
            <div>
              <p className="mb-1 text-2xs uppercase tracking-wide text-muted-foreground">Blocks</p>
              <div className="flex flex-col gap-1">{blocked.map((b) => refRow(b))}</div>
            </div>
          ) : null}
        </div>
      )}
    </section>
  );
}

/** Compact navigation card used on project pages to reach workload & guests. */
export function ProjectToolsLinks({ projectId }: { projectId: string }) {
  return (
    <div className="flex gap-2">
      <Link
        to={`/projects/${projectId}?view=workload`}
        className="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent"
      >
        <ListTree className="h-3.5 w-3.5" /> Workload
      </Link>
      <Link
        to={`/projects/${projectId}/guests`}
        className="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent"
      >
        <Users className="h-3.5 w-3.5" /> Guests
      </Link>
    </div>
  );
}
