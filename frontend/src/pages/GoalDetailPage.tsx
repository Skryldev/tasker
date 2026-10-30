import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  CheckCircle2,
  History,
  Pencil,
  Plus,
  Target,
  Trash2,
  TrendingDown,
  TrendingUp,
  XCircle,
} from "lucide-react";
import { api, ApiError, type Goal, type KeyResult } from "@/api/client";
import { Avatar, Separator } from "@/components/ui/avatar";
import { Skeleton } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input, Label } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { EmptyState, ErrorState } from "@/components/app/EmptyState";
import { useToast } from "@/components/ui/toast";
import { useWorkspace } from "@/lib/workspace";
import { cn, formatDate, timeAgo } from "@/lib/utils";
import { GoalFormDialog, GoalStatusBadge, ProgressBar } from "@/pages/GoalsPage";

type KrMetricType = "increase" | "decrease" | "boolean";

const METRIC_LABELS: Record<KrMetricType, string> = {
  increase: "Increase (toward a higher target)",
  decrease: "Decrease (toward a lower target)",
  boolean: "Boolean (done or not)",
};

const EVENT_LABELS: Record<string, string> = {
  goal_created: "created this goal",
  goal_updated: "updated this goal",
  goal_status_changed: "changed status",
  goal_deleted: "deleted this goal",
  goal_key_result_created: "added a key result",
  goal_key_result_updated: "updated a key result",
  goal_key_result_deleted: "deleted a key result",
  goal_project_associated: "associated a project",
  goal_project_disassociated: "removed a project association",
  goal_task_associated: "associated a task",
  goal_task_disassociated: "removed a task association",
};

/** Goal detail: metadata, lifecycle controls, key results, associations,
 *  execution stats (clearly separated from key-result progress). */
export function GoalDetailPage() {
  const { goalId } = useParams<{ goalId: string }>();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { push } = useToast();
  const { myRole } = useWorkspace();
  const canManage = myRole !== undefined && ["owner", "admin", "manager"].includes(myRole);

  const [editOpen, setEditOpen] = useState(false);
  const [krDialogOpen, setKrDialogOpen] = useState(false);
  const [editingKr, setEditingKr] = useState<KeyResult | null>(null);

  const detail = useQuery({
    queryKey: ["goal", goalId],
    queryFn: () => api.getGoal(goalId!),
    enabled: Boolean(goalId),
  });
  const activity = useQuery({
    queryKey: ["goal-activity", goalId],
    queryFn: () => api.goalActivity(goalId!),
    enabled: Boolean(goalId),
  });

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["goal", goalId] });
    void qc.invalidateQueries({ queryKey: ["goals"] });
    void qc.invalidateQueries({ queryKey: ["goal-activity", goalId] });
  };

  const transition = useMutation({
    mutationFn: (status: string) => api.transitionGoal(goalId!, status),
    onSuccess: () => {
      push("success", "Goal status updated");
      invalidate();
    },
    onError: (err) =>
      push("error", "Transition rejected", err instanceof Error ? err.message : undefined),
  });

  const removeKr = useMutation({
    mutationFn: (krId: string) => api.deleteKeyResult(krId),
    onSuccess: () => {
      push("info", "Key result deleted");
      invalidate();
    },
    onError: (err) =>
      push("error", "Could not delete key result", err instanceof Error ? err.message : undefined),
  });

  if (detail.isLoading) {
    return (
      <div className="mx-auto max-w-3xl p-6">
        <Skeleton className="mb-4 h-8 w-64" />
        <Skeleton className="mb-4 h-20 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }
  if (detail.isError) {
    return (
      <div className="mx-auto max-w-3xl p-6">
        <ErrorState
          title="Goal unavailable"
          detail="It may have been deleted, or you may not be a member of its team."
          onRetry={() => void detail.refetch()}
        />
      </div>
    );
  }

  const data = detail.data!;
  const goal: Goal = data.goal;
  const krs: KeyResult[] = data.key_results;
  const execution = data.execution;

  const confirmDelete = () => {
    if (!window.confirm(`Delete goal "${goal.title}"? Its key results and associations are removed.`)) {
      return;
    }
    api
      .deleteGoal(goal.id)
      .then(() => {
        push("info", "Goal deleted");
        void qc.invalidateQueries({ queryKey: ["goals"] });
        navigate(`/teams/${goal.team_id}/goals`);
      })
      .catch((err) =>
        push("error", "Could not delete goal", err instanceof Error ? err.message : undefined),
      );
  };

  return (
    <div className="mx-auto max-w-3xl p-6" data-testid="goal-detail">
      <div className="mb-4">
        <Button variant="ghost" size="sm" onClick={() => navigate(-1)}>
          <ArrowLeft /> Back
        </Button>
      </div>

      <header className="mb-5">
        <div className="mb-2 flex items-center gap-3">
          <Target className="h-5 w-5 text-primary" />
          <h1 className="text-lg font-semibold">{goal.title}</h1>
          <GoalStatusBadge status={goal.status} />
        </div>
        {goal.description ? (
          <p className="text-sm text-muted-foreground">{goal.description}</p>
        ) : null}
        <div className="mt-3 flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
          {goal.owner ? (
            <span className="inline-flex items-center gap-1.5">
              <Avatar name={goal.owner.display_name} size="xs" /> Owner:{" "}
              {goal.owner.display_name}
            </span>
          ) : null}
          {goal.start_date ? <span>Start {formatDate(goal.start_date)}</span> : null}
          {goal.due_date ? <span>Due {formatDate(goal.due_date)}</span> : null}
          <span>Created {timeAgo(goal.created_at)}</span>
        </div>

        {/* Measured progress — distinct from task execution stats below */}
        <div className="mt-4 rounded-lg border p-3.5" data-testid="goal-progress-section">
          <p className="mb-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground">
            Measured progress (key results)
          </p>
          {goal.progress !== undefined && goal.progress !== null ? (
            <ProgressBar progress={goal.progress} />
          ) : (
            <p className="text-xs italic text-muted-foreground">
              No key results yet — overall progress is undefined.
            </p>
          )}
        </div>

        {canManage ? (
          <div className="mt-3 flex flex-wrap items-center gap-2" data-testid="goal-lifecycle">
            <Button variant="outline" size="sm" onClick={() => setEditOpen(true)} data-testid="edit-goal">
              <Pencil /> Edit
            </Button>
            {goal.status === "draft" ? (
              <Button
                size="sm"
                onClick={() => transition.mutate("active")}
                disabled={transition.isPending}
                data-testid="activate-goal"
              >
                Activate
              </Button>
            ) : null}
            {goal.status === "active" ? (
              <Button
                size="sm"
                variant="outline"
                onClick={() => transition.mutate("completed")}
                disabled={transition.isPending}
                data-testid="complete-goal"
              >
                <CheckCircle2 /> Mark completed
              </Button>
            ) : null}
            {goal.status === "completed" || goal.status === "cancelled" ? (
              <Button
                size="sm"
                variant="outline"
                onClick={() => transition.mutate("active")}
                disabled={transition.isPending}
                data-testid="reopen-goal"
              >
                Reopen
              </Button>
            ) : null}
            {(goal.status === "draft" || goal.status === "active") ? (
              <Button
                size="sm"
                variant="ghost"
                onClick={() => transition.mutate("cancelled")}
                disabled={transition.isPending}
                className="text-destructive hover:bg-destructive/10"
                data-testid="cancel-goal"
              >
                <XCircle /> Cancel goal
              </Button>
            ) : null}
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto text-destructive hover:bg-destructive/10"
              onClick={confirmDelete}
              data-testid="delete-goal"
            >
              <Trash2 /> Delete
            </Button>
          </div>
        ) : null}
      </header>

      <Separator className="mb-6" />

      {/* ---------------- Key results ---------------- */}
      <section aria-label="Key results" className="mb-8">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">Key results ({krs.length})</h2>
          {canManage ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setEditingKr(null);
                setKrDialogOpen(true);
              }}
              data-testid="add-kr"
            >
              <Plus /> Add key result
            </Button>
          ) : null}
        </div>
        {krs.length === 0 ? (
          <EmptyState
            icon={<Target className="h-6 w-6" />}
            title="No key results"
            description="Key results make this goal measurable. Add at least one before activating."
          />
        ) : (
          <ul className="flex flex-col gap-3" data-testid="kr-list">
            {krs.map((kr) => (
              <li key={kr.id} className="rounded-lg border p-3.5" data-testid={`kr-${kr.id}`}>
                <div className="mb-1.5 flex items-start gap-2">
                  <span
                    className={cn(
                      "mt-0.5 inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-2xs font-medium",
                      kr.metric_type === "increase"
                        ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                        : kr.metric_type === "decrease"
                          ? "bg-sky-500/10 text-sky-600 dark:text-sky-400"
                          : "bg-violet-500/10 text-violet-600 dark:text-violet-400",
                    )}
                    title={`Metric: ${kr.metric_type}`}
                  >
                    {kr.metric_type === "increase" ? (
                      <TrendingUp className="h-3 w-3" />
                    ) : kr.metric_type === "decrease" ? (
                      <TrendingDown className="h-3 w-3" />
                    ) : (
                      <CheckCircle2 className="h-3 w-3" />
                    )}
                    {kr.metric_type}
                  </span>
                  <span className="min-w-0 flex-1 text-sm font-medium">{kr.title}</span>
                  {canManage ? (
                    <span className="flex shrink-0 gap-1">
                      <button
                        type="button"
                        aria-label={`Edit ${kr.title}`}
                        className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                        onClick={() => {
                          setEditingKr(kr);
                          setKrDialogOpen(true);
                        }}
                        data-testid={`edit-kr-${kr.id}`}
                      >
                        <Pencil className="h-3 w-3" />
                      </button>
                      <button
                        type="button"
                        aria-label={`Delete ${kr.title}`}
                        className="rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                        onClick={() => {
                          if (window.confirm(`Delete key result "${kr.title}"?`)) {
                            removeKr.mutate(kr.id);
                          }
                        }}
                        data-testid={`delete-kr-${kr.id}`}
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </span>
                  ) : null}
                </div>
                {kr.description ? (
                  <p className="mb-2 text-xs text-muted-foreground">{kr.description}</p>
                ) : null}
                <ProgressBar progress={kr.progress} />
                <p className="mt-1.5 text-2xs text-muted-foreground">
                  Current: {kr.current_value}
                  {kr.unit ? ` ${kr.unit}` : ""}
                  {kr.start_value !== null && kr.start_value !== undefined
                    ? ` · from ${kr.start_value}`
                    : ""}
                  {kr.target_value !== null && kr.target_value !== undefined
                    ? ` · target ${kr.target_value}`
                    : ""}
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>

      <Separator className="mb-6" />

      {/* ---------------- Associations + execution ---------------- */}
      <section aria-label="Contributing work" className="mb-8">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">Contributing projects &amp; tasks</h2>
          {canManage ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => navigate(`/projects?team=${goal.team_id}`)}
              data-testid="browse-projects"
            >
              Browse projects
            </Button>
          ) : null}
        </div>

        <div
          className="mb-4 rounded-lg border border-dashed p-3.5"
          data-testid="execution-summary"
        >
          <p className="text-2xs font-medium uppercase tracking-wide text-muted-foreground">
            Task execution progress (not goal achievement)
          </p>
          <p className="mt-1 text-sm">
            {execution.total} associated task{execution.total === 1 ? "" : "s"} ·{" "}
            {execution.completed} completed
            {execution.completion_percent !== null &&
            execution.completion_percent !== undefined
              ? ` (${Math.round(execution.completion_percent)}%)`
              : ""}
          </p>
          {Object.entries(execution.by_status).length > 0 ? (
            <div className="mt-1.5 flex flex-wrap gap-2">
              {Object.entries(execution.by_status).map(([status, count]) => (
                <span key={status} className="text-2xs text-muted-foreground">
                  {status.replaceAll("_", " ")}:{" "}
                  <span className="font-medium text-foreground">{count}</span>
                </span>
              ))}
            </div>
          ) : null}
        </div>

        {data.projects.length === 0 && data.tasks.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No projects or tasks associated yet.
          </p>
        ) : null}
        {data.projects.length > 0 ? (
          <>
            <h3 className="mb-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
              Projects
            </h3>
            <ul className="mb-4 flex flex-col gap-1.5" data-testid="goal-project-list">
              {data.projects.map((p) => (
                <li key={p.id}>
                  <Link
                    to={`/projects/${p.id}`}
                    className="flex items-center justify-between rounded-md border px-3 py-2 text-sm hover:bg-accent"
                  >
                    <span className="truncate">{p.name}</span>
                    <span className="text-2xs text-muted-foreground">{p.status}</span>
                  </Link>
                </li>
              ))}
            </ul>
          </>
        ) : null}
        {data.tasks.length > 0 ? (
          <>
            <h3 className="mb-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
              Tasks
            </h3>
            <ul className="flex flex-col gap-1.5" data-testid="goal-task-list">
              {data.tasks.map((t) => (
                <li key={t.id}>
                  <Link
                    to={`/tasks/${t.id}`}
                    className="flex items-center justify-between rounded-md border px-3 py-2 text-sm hover:bg-accent"
                  >
                    <span className="truncate">{t.title}</span>
                    <span className="text-2xs text-muted-foreground">
                      {t.status.replaceAll("_", " ")}
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          </>
        ) : null}
      </section>

      <Separator className="mb-6" />

      {/* ---------------- Activity ---------------- */}
      <section aria-label="Goal activity">
        <h2 className="mb-3 flex items-center gap-1.5 text-sm font-semibold">
          <History className="h-4 w-4" /> Activity
        </h2>
        {activity.isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : (
          <ol className="flex flex-col gap-2.5">
            {(activity.data?.items ?? []).map((event) => (
              <li key={event.id} className="flex items-start gap-2.5 text-xs">
                {event.actor ? (
                  <Avatar name={event.actor.display_name} size="xs" />
                ) : (
                  <span className="flex h-5 w-5 items-center justify-center rounded-full bg-muted text-[9px]">
                    ?
                  </span>
                )}
                <div>
                  <span className="font-medium">{event.actor?.display_name ?? "Someone"}</span>{" "}
                  <span className="text-muted-foreground">
                    {EVENT_LABELS[event.event] ?? event.event}
                  </span>
                  {event.new_value ? (
                    <div className="text-2xs text-muted-foreground">{event.new_value}</div>
                  ) : null}
                  <div className="text-2xs text-muted-foreground">{timeAgo(event.created_at)}</div>
                </div>
              </li>
            ))}
            {(activity.data?.items.length ?? 0) === 0 ? (
              <li className="text-xs text-muted-foreground">No activity yet.</li>
            ) : null}
          </ol>
        )}
      </section>

      {editOpen ? (
        <GoalFormDialog
          open={editOpen}
          onOpenChange={setEditOpen}
          teamId={goal.team_id}
          goal={goal}
        />
      ) : null}
      {krDialogOpen ? (
        <KrDialog
          open={krDialogOpen}
          onOpenChange={setKrDialogOpen}
          goalId={goal.id}
          existing={editingKr}
        />
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Key result add/edit dialog
// ---------------------------------------------------------------------------

function KrDialog({
  open,
  onOpenChange,
  goalId,
  existing,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  goalId: string;
  existing: KeyResult | null;
}) {
  const { push } = useToast();
  const qc = useQueryClient();
  const [title, setTitle] = useState(existing?.title ?? "");
  const [description] = useState(existing?.description ?? "");
  const [metricType, setMetricType] = useState<KrMetricType>(
    (existing?.metric_type as KrMetricType) ?? "increase",
  );
  const [startValue, setStartValue] = useState(
    existing?.start_value !== null && existing?.start_value !== undefined
      ? String(existing.start_value)
      : "",
  );
  const [currentValue, setCurrentValue] = useState(
    existing ? String(existing.current_value) : "",
  );
  const [targetValue, setTargetValue] = useState(
    existing?.target_value !== null && existing?.target_value !== undefined
      ? String(existing.target_value)
      : "",
  );
  const [unit, setUnit] = useState(existing?.unit ?? "");
  const [errors, setErrors] = useState<Record<string, string>>({});

  const save = useMutation({
    mutationFn: () => {
      const num = (v: string): number | undefined => (v === "" ? undefined : Number(v));
      const body = {
        title: title.trim(),
        description: description || undefined,
        metric_type: metricType,
        start_value: metricType === "boolean" ? undefined : num(startValue),
        current_value: metricType === "boolean" ? Number(currentValue === "1" ? 1 : 0) : num(currentValue) ?? undefined,
        target_value: metricType === "boolean" ? undefined : num(targetValue),
        unit: unit || undefined,
      };
      return existing
        ? api.updateKeyResult(existing.id, body)
        : api.createKeyResult(goalId, body);
    },
    onSuccess: () => {
      push("success", existing ? "Key result updated" : "Key result added");
      void qc.invalidateQueries({ queryKey: ["goal", goalId] });
      void qc.invalidateQueries({ queryKey: ["goals"] });
      onOpenChange(false);
    },
    onError: (err) => {
      if (err instanceof ApiError && err.problem.errors) {
        const map: Record<string, string> = {};
        for (const e of err.problem.errors) map[e.field] = e.message;
        setErrors(map);
      } else {
        push("error", "Could not save key result", err instanceof Error ? err.message : undefined);
      }
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{existing ? "Edit key result" : "New key result"}</DialogTitle>
          <DialogDescription>
            A measurable outcome. Progress is computed from the values — it is
            never entered manually.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            setErrors({});
            if (!title.trim()) {
              setErrors({ title: "Title is required" });
              return;
            }
            if (metricType !== "boolean" && (!startValue || !targetValue)) {
              setErrors({ start_value: "Start and target values are required" });
              return;
            }
            save.mutate();
          }}
          noValidate
          data-testid="kr-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="kr-title">Title</Label>
            <Input
              id="kr-title"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Active users 100 → 500"
              autoFocus
            />
            {errors.title ? (
              <p className="text-2xs text-destructive" role="alert">{errors.title}</p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="kr-metric">Metric type</Label>
            <Select
              value={metricType}
              onValueChange={(v) => setMetricType(v as KrMetricType)}
            >
              <SelectTrigger id="kr-metric">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(Object.keys(METRIC_LABELS) as KrMetricType[]).map((t) => (
                  <SelectItem key={t} value={t}>
                    {METRIC_LABELS[t]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {metricType === "boolean" ? (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="kr-achieved">Achieved?</Label>
              <Select
                value={currentValue === "1" ? "1" : "0"}
                onValueChange={(v) => setCurrentValue(v)}
              >
                <SelectTrigger id="kr-achieved">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="0">Not achieved (0%)</SelectItem>
                  <SelectItem value="1">Achieved (100%)</SelectItem>
                </SelectContent>
              </Select>
            </div>
          ) : (
            <div className="grid grid-cols-3 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="kr-start">Start</Label>
                <Input
                  id="kr-start"
                  type="number"
                  step="any"
                  value={startValue}
                  onChange={(e) => setStartValue(e.target.value)}
                />
                {errors.start_value ? (
                  <p className="text-2xs text-destructive" role="alert">{errors.start_value}</p>
                ) : null}
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="kr-current">Current</Label>
                <Input
                  id="kr-current"
                  type="number"
                  step="any"
                  value={currentValue}
                  onChange={(e) => setCurrentValue(e.target.value)}
                />
                {errors.current_value ? (
                  <p className="text-2xs text-destructive" role="alert">{errors.current_value}</p>
                ) : null}
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="kr-target">Target</Label>
                <Input
                  id="kr-target"
                  type="number"
                  step="any"
                  value={targetValue}
                  onChange={(e) => setTargetValue(e.target.value)}
                />
                {errors.target_value ? (
                  <p className="text-2xs text-destructive" role="alert">{errors.target_value}</p>
                ) : null}
              </div>
            </div>
          )}
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="kr-unit">Unit (optional)</Label>
            <Input
              id="kr-unit"
              value={unit}
              onChange={(e) => setUnit(e.target.value)}
              placeholder="e.g. users, %, ms"
            />
            {errors.unit ? (
              <p className="text-2xs text-destructive" role="alert">{errors.unit}</p>
            ) : null}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={save.isPending} data-testid="kr-submit">
              {save.isPending ? "Saving…" : existing ? "Save changes" : "Add key result"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
