import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Target } from "lucide-react";
import { api, ApiError, type Goal, type GoalStatus } from "@/api/client";
import { Avatar } from "@/components/ui/avatar";
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
import { Input, Label, Textarea } from "@/components/ui/input";
import { DateField } from "@/components/ui/date-field";
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
import { cn } from "@/lib/utils";
import { formatJalaliDate } from "@/lib/doran";

const GOAL_STATUS_LABELS: Record<GoalStatus, string> = {
  draft: "Draft",
  active: "Active",
  completed: "Completed",
  cancelled: "Cancelled",
};

const STATUS_TONE: Record<GoalStatus, string> = {
  draft: "bg-muted text-muted-foreground",
  active: "bg-info/10 text-info",
  completed: "bg-success/10 text-success",
  cancelled: "bg-destructive/10 text-destructive",
};

export function GoalStatusBadge({ status }: { status: string }) {
  const s = status as GoalStatus;
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full px-2 py-0.5 text-2xs font-medium",
        STATUS_TONE[s] ?? "bg-muted text-muted-foreground",
      )}
      data-testid={`goal-status-${status}`}
    >
      {GOAL_STATUS_LABELS[s] ?? status}
    </span>
  );
}

export function ProgressBar({ progress }: { progress: number }) {
  const clamped = Math.min(100, Math.max(0, progress));
  return (
    <div className="flex items-center gap-2" data-testid="goal-progress">
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div
          className="h-full rounded-full bg-primary transition-all"
          style={{ width: `${clamped}%` }}
          role="progressbar"
          aria-valuenow={Math.round(clamped)}
          aria-valuemin={0}
          aria-valuemax={100}
        />
      </div>
      <span className="shrink-0 text-2xs font-medium">{Math.round(clamped)}%</span>
    </div>
  );
}

type StatusFilter = "all" | GoalStatus;

/** Team goals list — Manager+ creates; all members view. */
export function GoalsPage() {
  const { teamId: routeTeam } = useParams<{ teamId: string }>();
  const { teamId: workspaceTeam } = useWorkspace();
  const teamId = routeTeam ?? workspaceTeam;
  const { myRole } = useWorkspace();
  const canManage = myRole !== undefined && ["owner", "admin", "manager"].includes(myRole);
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [createOpen, setCreateOpen] = useState(false);

  const goals = useQuery({
    queryKey: ["goals", teamId ?? "", statusFilter],
    queryFn: () => api.listGoals(teamId!, statusFilter === "all" ? undefined : statusFilter),
    enabled: Boolean(teamId),
  });

  if (!teamId) {
    return (
      <EmptyState
        icon={<Target className="h-8 w-8" />}
        title="No team selected"
        description="Select a team to see its goals."
      />
    );
  }

  return (
    <div className="mx-auto max-w-3xl p-6" data-testid="goals-page">
      <header className="mb-5 flex items-center justify-between gap-3">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <Target className="h-5 w-5 text-primary" /> Goals
          </h1>
          <p className="text-xs text-muted-foreground">
            Team objectives with measurable key results.
          </p>
        </div>
        {canManage ? (
          <Button onClick={() => setCreateOpen(true)} data-testid="create-goal">
            <Target className="h-4 w-4" /> New goal
          </Button>
        ) : null}
      </header>

      <div className="mb-4 flex flex-wrap gap-1" role="tablist" aria-label="Filter by status">
        {(["all", "draft", "active", "completed", "cancelled"] as const).map((s) => (
          <button
            key={s}
            role="tab"
            aria-selected={statusFilter === s}
            onClick={() => setStatusFilter(s)}
            className={cn(
              "rounded-full px-3 py-1 text-xs font-medium transition-colors",
              statusFilter === s
                ? "bg-primary text-primary-foreground"
                : "bg-muted text-muted-foreground hover:text-foreground",
            )}
            data-testid={`filter-${s}`}
          >
            {s === "all" ? "All" : GOAL_STATUS_LABELS[s]}
          </button>
        ))}
      </div>

      {goals.isLoading ? (
        <div className="flex flex-col gap-3">
          {[0, 1].map((i) => (
            <Skeleton key={i} className="h-28 w-full" />
          ))}
        </div>
      ) : goals.isError ? (
        <ErrorState title="Could not load goals" onRetry={() => void goals.refetch()} />
      ) : (goals.data?.items.length ?? 0) === 0 ? (
        <EmptyState
          icon={<Target className="h-8 w-8" />}
          title={statusFilter === "all" ? "No goals yet" : `No ${GOAL_STATUS_LABELS[statusFilter].toLowerCase()} goals`}
          description={
            canManage
              ? "Create a team objective and measure it with key results."
              : "Managers can create team objectives here."
          }
        />
      ) : (
        <ul className="flex flex-col gap-3" data-testid="goal-list">
          {goals.data!.items.map((goal) => (
            <li key={goal.id}>
              <Link
                to={`/goals/${goal.id}`}
                className="block rounded-lg border bg-card p-4 transition-colors hover:border-primary/40"
                data-testid={`goal-card-${goal.id}`}
              >
                <div className="mb-2 flex items-center gap-2">
                  <span className="truncate text-sm font-semibold">{goal.title}</span>
                  <GoalStatusBadge status={goal.status} />
                </div>
                {goal.description ? (
                  <p className="mb-2 line-clamp-1 text-xs text-muted-foreground">
                    {goal.description}
                  </p>
                ) : null}
                <div className="flex flex-wrap items-center gap-3 text-2xs text-muted-foreground">
                  {goal.owner ? (
                    <span className="inline-flex items-center gap-1">
                      <Avatar name={goal.owner.display_name} size="xs" />
                      {goal.owner.display_name}
                    </span>
                  ) : null}
                  {goal.due_date ? <span>Due {formatJalaliDate(goal.due_date)}</span> : null}
                  <span>
                    {goal.key_result_count ?? 0} key result{" "}
                    {(goal.key_result_count ?? 0) === 1 ? "" : "s"}
                  </span>
                </div>
                {goal.progress !== undefined && goal.progress !== null ? (
                  <div className="mt-2">
                    <ProgressBar progress={goal.progress} />
                  </div>
                ) : (
                  <p className="mt-2 text-2xs italic text-muted-foreground">
                    No key results — progress undefined
                  </p>
                )}
              </Link>
            </li>
          ))}
        </ul>
      )}

      {createOpen && teamId ? (
        <GoalFormDialog open={createOpen} onOpenChange={setCreateOpen} teamId={teamId} />
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Create/edit dialog (shared by list + detail pages)
// ---------------------------------------------------------------------------

export function GoalFormDialog({
  open,
  onOpenChange,
  teamId,
  goal,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  teamId: string;
  goal?: Goal;
}) {
  const { push } = useToast();
  const qc = useQueryClient();
  const members = useQuery({
    queryKey: ["members", teamId],
    queryFn: async () => {
      const res = await fetch(`/api/v1/teams/${teamId}/members`, {
        credentials: "same-origin",
      });
      if (!res.ok) throw new Error("members unavailable");
      return res.json() as Promise<{ items: { user: { id: string; display_name: string; email: string } }[] }>;
    },
    enabled: open && Boolean(teamId),
  });

  const [title, setTitle] = useState(goal?.title ?? "");
  const [description, setDescription] = useState(goal?.description ?? "");
  const [ownerId, setOwnerId] = useState(goal?.owner?.id ?? "caller");
  const [startDate, setStartDate] = useState(goal?.start_date ?? "");
  const [dueDate, setDueDate] = useState(goal?.due_date ?? "");
  const [errors, setErrors] = useState<Record<string, string>>({});

  const save = useMutation({
    mutationFn: async () => {
      const body = {
        title: title.trim(),
        description: description || undefined,
        owner_id: ownerId === "caller" ? undefined : ownerId,
        start_date: startDate || undefined,
        due_date: dueDate || undefined,
      };
      if (goal) return api.updateGoal(goal.id, body);
      return api.createGoal(teamId, body);
    },
    onSuccess: () => {
      push("success", goal ? "Goal updated" : "Goal created");
      void qc.invalidateQueries({ queryKey: ["goals"] });
      void qc.invalidateQueries({ queryKey: ["goal", goal?.id] });
      onOpenChange(false);
    },
    onError: (err) => {
      if (err instanceof ApiError && err.problem.errors) {
        const map: Record<string, string> = {};
        for (const e of err.problem.errors) map[e.field] = e.message;
        setErrors(map);
      } else {
        push("error", "Could not save goal", err instanceof Error ? err.message : undefined);
      }
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{goal ? "Edit goal" : "New goal"}</DialogTitle>
          <DialogDescription>
            Objectives start in Draft. Add key results before activating.
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
            save.mutate();
          }}
          noValidate
          data-testid="goal-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="goal-title">Title</Label>
            <Input
              id="goal-title"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="e.g. Grow activation"
              autoFocus
              aria-invalid={Boolean(errors.title)}
            />
            {errors.title ? (
              <p className="text-2xs text-destructive" role="alert">{errors.title}</p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="goal-description">Description</Label>
            <Textarea
              id="goal-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={3}
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="goal-start">Start date</Label>
              <DateField
                id="goal-start"
                value={startDate || null}
                onChange={setStartDate}
                ariaLabel="Start date"
                invalid={Boolean(errors.start_date)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="goal-due">Due date</Label>
              <DateField
                id="goal-due"
                value={dueDate || null}
                onChange={setDueDate}
                ariaLabel="Due date"
                invalid={Boolean(errors.due_date)}
              />
              {errors.due_date ? (
                <p className="text-2xs text-destructive" role="alert">{errors.due_date}</p>
              ) : null}
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="goal-owner">Owner</Label>
            <Select value={ownerId || "caller"} onValueChange={setOwnerId}>
              <SelectTrigger id="goal-owner">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="caller">Me</SelectItem>
                {(members.data?.items ?? []).map((m) => (
                  <SelectItem key={m.user.id} value={m.user.id}>
                    {m.user.display_name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {errors.owner_id ? (
            <p className="text-2xs text-destructive" role="alert">{errors.owner_id}</p>
          ) : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={save.isPending} data-testid="goal-submit">
              {save.isPending ? "Saving…" : goal ? "Save changes" : "Create goal"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
