import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Check,
  ChevronRight,
  Copy,
  Lock,
  MoreHorizontal,
  Pencil,
  Link2,
  Trash2,
} from "lucide-react";
import { lazy, Suspense } from "react";
import { api, ApiError, type Task } from "@/api/client";
import {
  useDeleteTask,
  useUpdateTask,
} from "@/api/hooks";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ErrorState } from "@/components/app/EmptyState";
import { MetadataBand, TaskDetailBody, useUpdateTaskBody } from "@/components/app/TaskDetailPanel";
import { StatusControl } from "@/components/app/StatusControl";
import { Timestamp } from "@/components/app/Timestamp";
import { useToast } from "@/components/ui/toast";

// Bug 4: the edit dialog pulls the Doran stack; loaded on first open so the
// eager task views do not carry it (Phase 10 rule).
const TaskDialog = lazy(() => import("@/components/app/TaskDialog").then((m) => ({ default: m.TaskDialog })));

/**
 * Bug 4 (Phase 12.5): the task detail as a dedicated, Staff-level page —
 * two columns on desktop (content + metadata rail), one on mobile. The
 * quick-view panel and this page render the SAME shared body
 * (`TaskDetailBody`) and metadata band (`MetadataBand`), so the surfaces
 * cannot drift.
 *
 * Routes: the canonical `/teams/:teamId/projects/:projectId/tasks/:taskId`
 * and the legacy `/tasks/:taskId` (notifications), which resolves the
 * team/project from the task itself.
 */

export function TaskPage() {
  const params = useParams<{
    taskId: string;
    teamId?: string;
    projectId?: string;
  }>();
  const taskId = params.taskId;
  const query = useQuery({
    queryKey: ["task", taskId],
    queryFn: () => api.getTask(taskId!),
    retry: (count, error) => error instanceof ApiError && error.status >= 500 && count < 2,
    enabled: Boolean(taskId),
  });

  if (!taskId || query.isLoading) {
    return (
      <div className="mx-auto max-w-6xl p-6" aria-busy="true">
        <div className="mb-4 h-5 w-64 animate-pulse rounded bg-muted" />
        <div className="h-24 animate-pulse rounded bg-muted" />
      </div>
    );
  }
  if (query.isError || !query.data) {
    return (
      <div className="p-6">
        <ErrorState
          title="This task is no longer available"
          detail="It may have been deleted, or you no longer have access to its team."
          onRetry={() => void query.refetch()}
        />
      </div>
    );
  }

  const task = query.data;
  return (
    <TaskPageInner
      key={task.id}
      task={task}
      routeTeamId={params.teamId}
      routeProjectId={params.projectId}
    />
  );
}

function TaskPageInner({
  task: initialTask,
  routeTeamId,
  routeProjectId,
}: {
  task: Task;
  routeTeamId?: string;
  routeProjectId?: string;
}) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { push } = useToast();
  const deleteTask = useDeleteTask();
  const updateTask = useUpdateTask();
  const updateTaskBody = useUpdateTaskBody();

  const [editOpen, setEditOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [titleEditing, setTitleEditing] = useState(false);
  const [titleDraft, setTitleDraft] = useState(initialTask.title);
  const [duplicating, setDuplicating] = useState(false);

  const project = useQuery({
    queryKey: ["project", initialTask.project_id],
    queryFn: () => api.getProject(initialTask.project_id),
    enabled: Boolean(initialTask.project_id),
  });
  const projectName = project.data?.name ?? "Project";

  const isClosed = initialTask.status === "closed";
  const teamId = routeTeamId ?? initialTask.team_id;
  const projectId = routeProjectId ?? initialTask.project_id;

  const saveTitle = () => {
    setTitleEditing(false);
    const next = titleDraft.trim();
    if (!next || next === initialTask.title) return;
    updateTask.mutate(
      { taskId: initialTask.id, body: { title: next } },
      {
        onSuccess: (updated) => {
          void qc.setQueryData(["task", updated.id], updated);
          push("success", "Title updated");
        },
        onError: (error) =>
          push("error", "Could not rename task", error instanceof Error ? error.message : undefined),
      },
    );
  };

  const duplicate = async () => {
    setDuplicating(true);
    try {
      const creationStatus = (
        ["backlog", "todo", "in_progress"] as const
      ).includes(initialTask.status as "backlog")
        ? (initialTask.status as "backlog" | "todo" | "in_progress")
        : "todo";
      const copy = await api.createTask(projectId, {
        title: `${initialTask.title} (copy)`,
        description: initialTask.description ?? undefined,
        status: creationStatus,
        priority: initialTask.priority,
        assignee_id: initialTask.assignee?.id,
        due_date: initialTask.due_date ?? undefined,
        deadline: initialTask.deadline ?? undefined,
        label_ids: initialTask.labels.map((l) => l.id),
      });
      push("success", "Task duplicated");
      void qc.invalidateQueries({ queryKey: ["board"] });
      void qc.invalidateQueries({ queryKey: ["tasks"] });
      navigate(`/teams/${teamId}/projects/${projectId}/tasks/${copy.id}`);
    } catch (error) {
      push("error", "Could not duplicate task", error instanceof Error ? error.message : undefined);
    } finally {
      setDuplicating(false);
    }
  };

  const copyLink = () => {
    const url = `${window.location.origin}/teams/${teamId}/projects/${projectId}/tasks/${initialTask.id}`;
    void navigator.clipboard
      ?.writeText(url)
      .then(() => push("success", "Link copied"))
      .catch(() => push("error", "Could not copy the link"));
  };

  const confirmDeleteTask = () => {
    deleteTask.mutate(initialTask.id, {
      onSuccess: () => {
        push("info", "Task deleted");
        setDeleteOpen(false);
        void qc.invalidateQueries({ queryKey: ["board"] });
        void qc.invalidateQueries({ queryKey: ["tasks"] });
        navigate(`/projects/${projectId}?team=${teamId}`);
      },
      onError: (error) =>
        push("error", "Could not delete task", error instanceof Error ? error.message : undefined),
    });
  };

  return (
    <div className="flex h-full flex-col" data-testid="task-page">
      {/* Sticky header: back, breadcrumb, status, overflow. */}
      <header className="shrink-0 border-b bg-card/60 px-4 py-3 backdrop-blur">
        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Back to the project"
            onClick={() => navigate(-1)}
            data-testid="task-page-back"
          >
            <ArrowLeft />
          </Button>
          <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
            <Link to={`/teams/${teamId}`} className="truncate hover:text-foreground hover:underline">
              Team
            </Link>
            <ChevronRight className="h-3 w-3 shrink-0" aria-hidden />
            <Link
              to={`/projects/${projectId}?team=${teamId}`}
              className="truncate hover:text-foreground hover:underline"
            >
              {projectName}
            </Link>
            <ChevronRight className="h-3 w-3 shrink-0" aria-hidden />
            <span className="truncate font-medium text-foreground">{initialTask.title}</span>
          </nav>
          <div className="ml-auto flex items-center gap-2">
            <StatusControl task={initialTask} />
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="icon-sm" aria-label="Task actions" data-testid="task-actions">
                  <MoreHorizontal />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-44">
                {!isClosed ? (
                  <DropdownMenuItem onSelect={() => setEditOpen(true)} data-testid="edit-task">
                    <Pencil /> Edit fields…
                  </DropdownMenuItem>
                ) : null}
                <DropdownMenuItem onSelect={() => void duplicate()} disabled={duplicating} data-testid="duplicate-task">
                  <Copy /> Duplicate task
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={copyLink} data-testid="copy-link">
                  <Link2 /> Copy link
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  destructive
                  onSelect={() => setDeleteOpen(true)}
                  data-testid="delete-task"
                >
                  <Trash2 /> Delete task
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </header>

      {/* Two columns on desktop; the metadata rail drops below on mobile. */}
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto scrollbar-thin lg:flex-row lg:overflow-hidden">
        {/* Same testid as the quick view's content root: the shared body
            renders identically on both surfaces (specs target it either way). */}
        <div
          className="min-h-0 flex-1 overflow-y-auto scrollbar-thin p-4 lg:p-6"
          data-testid="task-detail"
        >
          <div className="mb-4">
            <p className="flex items-center gap-1.5 font-mono text-2xs text-muted-foreground">
              Task · {initialTask.id.slice(0, 8)}
              {isClosed ? (
                <span
                  className="inline-flex items-center gap-1 rounded-full border border-muted-foreground/30 bg-muted px-1.5 py-0.5 font-sans text-2xs font-medium"
                  data-testid="task-locked"
                >
                  <Lock className="h-2.5 w-2.5" aria-hidden /> Locked
                </span>
              ) : null}
            </p>
            {titleEditing ? (
              <div className="mt-1 flex items-center gap-2">
                <Input
                  value={titleDraft}
                  onChange={(e) => setTitleDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") saveTitle();
                    if (e.key === "Escape") setTitleEditing(false);
                  }}
                  autoFocus
                  aria-label="Task title"
                  data-testid="task-page-title-input"
                />
                <Button variant="ghost" size="icon-sm" aria-label="Save title" onClick={saveTitle}>
                  <Check />
                </Button>
              </div>
            ) : (
              <h1 className="mt-0.5 flex items-start gap-1.5 text-xl font-semibold leading-snug">
                <button
                  type="button"
                  onClick={() => {
                    setTitleDraft(initialTask.title);
                    setTitleEditing(true);
                  }}
                  disabled={isClosed}
                  className="min-w-0 flex-1 text-left hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:hover:bg-transparent"
                  aria-label="Rename task"
                  data-testid="task-page-title"
                >
                  {initialTask.title}
                </button>
              </h1>
            )}
          </div>

          <TaskDetailBody
            task={initialTask}
            readOnly={isClosed}
            onChange={(body, label) => updateTaskBody(initialTask.id, body, label)}
          />
        </div>

        {/* Metadata rail: the same shared MetadataBand the quick view uses. */}
        <aside
          className="shrink-0 border-t bg-muted/20 p-4 lg:w-[340px] lg:border-l lg:border-t-0"
          data-testid="task-page-rail"
          aria-label="Task metadata"
        >
          <MetadataBand
            task={initialTask}
            readOnly={isClosed}
            onChange={(body, label) => updateTaskBody(initialTask.id, body, label)}
          />
          <dl className="mt-5 flex flex-col gap-1.5 text-2xs text-muted-foreground">
            <div className="flex justify-between gap-2">
              <dt>Status</dt>
              <dd className="text-foreground">{initialTask.status.replaceAll("_", " ")}</dd>
            </div>
            <div className="flex justify-between gap-2">
              <dt>Created</dt>
              <dd className="text-foreground"><Timestamp value={initialTask.created_at} /></dd>
            </div>
            <div className="flex justify-between gap-2">
              <dt>Updated</dt>
              <dd className="text-foreground"><Timestamp value={initialTask.updated_at} /></dd>
            </div>
          </dl>
        </aside>
      </div>

      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title="Delete this task?"
        description={`"${initialTask.title}" and its subtasks are permanently removed.`}
        confirmLabel="Delete task"
        pending={deleteTask.isPending}
        onConfirm={confirmDeleteTask}
      />

      {editOpen ? (
        <Suspense fallback={null}>
          <TaskDialog
            open={editOpen}
            onOpenChange={setEditOpen}
            teamId={teamId}
            projectId={projectId}
            task={initialTask}
          />
        </Suspense>
      ) : null}
    </div>
  );
}
