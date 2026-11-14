import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, Play, SendHorizonal, Undo2 } from "lucide-react";
import { api, ApiError, type Task } from "@/api/client";
import { useAuth } from "@/lib/auth";
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
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Label, Textarea } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";
import { STATUS_LABELS, type TaskStatus } from "@/lib/utils";
import { cn } from "@/lib/utils";

/**
 * StatusControl (Phase 10.5 §3.2): the professional replacement for the
 * single-button workflow actions.
 *
 * Left: the current status as a dot + label pill that opens a transition
 * menu — every lifecycle state is listed; the ones the backend would accept
 * for this user are enabled, everything else is disabled with a native
 * tooltip explaining why. The backend remains authoritative: the menu only
 * *predicts* what the API accepts, it cannot bypass it.
 *
 * Right: at most two contextual primary actions (assignee pickup / resolve /
 * submit, or accept + reject for the reviewer). Accept shows a confirmation
 * with consequences; reject requires a reason (Phase 2 backend contract).
 *
 * Motion: 150ms ease-out transitions only (button component contract);
 * dialogs/dropdowns animate via the shared Radix primitives.
 */

const STATUS_DOT: Record<TaskStatus, string> = {
  backlog: "bg-status-backlog",
  todo: "bg-status-todo",
  in_progress: "bg-status-in-progress",
  in_review: "bg-status-in-review",
  resolved: "bg-status-resolved",
  closed: "bg-status-closed",
};

/** Why a (task, actor) pair cannot move to `target` — shown as the tooltip. */
function transitionBlocker(
  task: Task,
  isAssignee: boolean,
  isReporter: boolean,
  target: TaskStatus,
): string | null {
  if (target === task.status) return "This is the current status";
  const pair = `${task.status}->${target}` as const;
  const actorOk =
    (pair === "backlog->todo" ||
      pair === "todo->in_progress" ||
      pair === "in_progress->resolved")
      ? isAssignee
      : pair === "resolved->in_review" || pair === "in_review->closed" || pair === "in_review->in_progress"
        ? isReporter && !isAssignee
        : false;
  if (!actorOk) {
    return pair === "in_review->closed"
      ? "Only the reporter can accept this task"
      : pair === "resolved->in_review"
        ? "Only the reporter can submit this task for review"
        : pair === "in_review->in_progress"
          ? "Only the reporter can reject this task"
          : "Only the assignee can perform this transition";
  }
  return null;
}

const LEGAL_TARGETS: Record<TaskStatus, TaskStatus[]> = {
  backlog: ["todo"],
  todo: ["in_progress"],
  in_progress: ["resolved"],
  resolved: ["in_review"],
  in_review: ["closed", "in_progress"],
  closed: [],
};

export function StatusControl({ task }: { task: Task }) {
  const { user } = useAuth();
  const { push } = useToast();
  const qc = useQueryClient();
  const [rejectOpen, setRejectOpen] = useState(false);
  const [acceptOpen, setAcceptOpen] = useState(false);
  const [blockersOpen, setBlockersOpen] = useState(false);
  const [blockersNote, setBlockersNote] = useState<string | null>(null);
  const [overrideReason, setOverrideReason] = useState("");
  const [overrideError, setOverrideError] = useState<string | null>(null);
  const [reason, setReason] = useState("");
  const [reasonError, setReasonError] = useState<string | null>(null);

  const isAssignee = task.assignee?.id === user?.id;
  const isReporter = task.reporter?.id === user?.id;

  const refresh = (updated: Task) => {
    void qc.setQueryData(["task", updated.id], updated);
    void qc.invalidateQueries({ queryKey: ["board"] });
    void qc.invalidateQueries({ queryKey: ["tasks"] });
    void qc.invalidateQueries({ queryKey: ["my-tasks"] });
    void qc.invalidateQueries({ queryKey: ["notifications"] });
    void qc.invalidateQueries({ queryKey: ["activity", updated.id] });
  };

  const onError = (error: unknown) => {
    push(
      "error",
      "Action not allowed",
      error instanceof ApiError ? error.message : "The task state may have changed; refresh and retry.",
    );
  };

  const transition = (to: string, why?: string) => api.transitionTask(task.id, to, why);

  const startProgress = useMutation({
    mutationFn: () => transition(task.status === "backlog" ? "todo" : "in_progress"),
    onSuccess: refresh,
    onError,
  });
  const resolve = useMutation({
    mutationFn: () => transition("resolved"),
    onSuccess: refresh,
    onError,
  });
  const submit = useMutation({
    mutationFn: () => transition("in_review"),
    onSuccess: refresh,
    onError,
  });
  const accept = useMutation({
    mutationFn: () => transition("closed"),
    onSuccess: (updated) => {
      setAcceptOpen(false);
      refresh(updated);
    },
    onError: (error) => {
      // Phase 14 WS6 (ADR-0023): the server refuses to close a task with
      // open `blocks` dependencies unless an override reason is given —
      // surface the blocker modal instead of a dead-end toast.
      if (error instanceof ApiError && error.status === 409) {
        setAcceptOpen(false);
        setBlockersNote(error.message);
        setBlockersOpen(true);
        return;
      }
      onError(error);
    },
  });
  const acceptWithOverride = useMutation({
    mutationFn: (why: string) => transition("closed", why),
    onSuccess: (updated) => {
      setBlockersOpen(false);
      setOverrideReason("");
      refresh(updated);
    },
    onError,
  });
  const reject = useMutation({
    mutationFn: () => transition("in_progress", reason.trim()),
    onSuccess: (updated) => {
      setRejectOpen(false);
      setReason("");
      refresh(updated);
    },
    onError,
  });

  const perform = (target: TaskStatus) => {
    if (target === "in_progress" && task.status === "in_review") {
      setRejectOpen(true);
      return;
    }
    if (target === "closed") {
      setAcceptOpen(true);
      return;
    }
    if (task.status === "backlog" || task.status === "todo") startProgress.mutate();
    else if (target === "resolved") resolve.mutate();
    else if (target === "in_review") submit.mutate();
  };

  const pending =
    startProgress.isPending || resolve.isPending || submit.isPending || accept.isPending || reject.isPending;

  const status = task.status as TaskStatus;
  const targets = LEGAL_TARGETS[status] ?? [];

  // Contextual primary actions (max 2) + the menu for everything else.
  const actions: { testId: string; label: string; icon: React.ReactNode; onClick: () => void; tone: "default" | "destructive" }[] = [];
  if (isAssignee && (status === "backlog" || status === "todo")) {
    actions.push({
      testId: "start-task",
      label: status === "backlog" ? "Pick up" : "Start work",
      icon: <Play />,
      onClick: () => startProgress.mutate(),
      tone: "default",
    });
  }
  if (isAssignee && status === "in_progress") {
    actions.push({
      testId: "resolve-task",
      label: "Mark resolved",
      icon: <SendHorizonal />,
      onClick: () => resolve.mutate(),
      tone: "default",
    });
  }
  if (status === "resolved" && isReporter && !isAssignee) {
    actions.push({
      testId: "submit-task",
      label: "Submit for review",
      icon: <SendHorizonal />,
      onClick: () => submit.mutate(),
      tone: "default",
    });
  }
  if (status === "in_review" && isReporter && !isAssignee) {
    actions.push({
      testId: "accept-task",
      label: "Accept & Close",
      icon: <Check />,
      onClick: () => setAcceptOpen(true),
      tone: "default",
    });
    actions.push({
      testId: "reject-task",
      label: "Reject",
      icon: <Undo2 />,
      onClick: () => setRejectOpen(true),
      tone: "destructive",
    });
  }

  return (
    <div
      className="flex flex-wrap items-center justify-between gap-2"
      data-testid="status-control"
      data-status={status}
    >
      {/* Current status + transition menu */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="sm"
            className="gap-2 font-medium"
            aria-label={`Status: ${STATUS_LABELS[status]}. Open transition menu`}
            data-testid="status-control-trigger"
            disabled={pending}
          >
            <span className={cn("h-1.5 w-1.5 rounded-full", STATUS_DOT[status])} aria-hidden />
            {STATUS_LABELS[status]}
            <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" aria-hidden />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          <DropdownMenuLabel>Move to</DropdownMenuLabel>
          {targets.length === 0 ? (
            <DropdownMenuItem disabled className="text-2xs">
              Closed is a terminal state
            </DropdownMenuItem>
          ) : null}
          {(Object.keys(STATUS_LABELS) as TaskStatus[]).map((target) => {
            const blocker = transitionBlocker(task, isAssignee, isReporter, target);
            const isCurrent = target === status;
            const isTarget = targets.includes(target) && !blocker;
            return (
              <DropdownMenuItem
                key={target}
                disabled={isCurrent || !isTarget}
                title={isCurrent ? "This is the current status" : (blocker ?? undefined)}
                onSelect={() => perform(target)}
                data-testid={`transition-${target.replaceAll("_", "-")}`}
                className={cn("justify-between", target === "in_progress" && status === "in_review" && "text-destructive focus:text-destructive")}
              >
                <span className="flex items-center gap-2">
                  <span className={cn("h-1.5 w-1.5 rounded-full", STATUS_DOT[target])} aria-hidden />
                  {STATUS_LABELS[target]}
                </span>
                {isCurrent ? <Check className="h-3.5 w-3.5" /> : null}
              </DropdownMenuItem>
            );
          })}
          <DropdownMenuSeparator />
          <p className="px-2 py-1.5 text-2xs text-muted-foreground">
            The server validates every transition against the workflow and your role.
          </p>
        </DropdownMenuContent>
      </DropdownMenu>

      {/* Contextual actions (max two) */}
      <div className="flex flex-wrap items-center gap-2">
        {actions.map((a) => (
          <Button
            key={a.testId}
            size="sm"
            variant={a.tone}
            onClick={a.onClick}
            disabled={pending}
            data-testid={a.testId}
          >
            {a.icon} {a.label}
          </Button>
        ))}
        {status === "in_review" && isReporter && isAssignee ? (
          <p className="text-2xs text-muted-foreground">
            This task has overlapping assignee and reporter; the workflow needs distinct users.
          </p>
        ) : null}
      </div>

      <AcceptDialog
        open={acceptOpen}
        onOpenChange={setAcceptOpen}
        pending={accept.isPending}
        assigneeName={task.assignee?.display_name}
        onSubmit={() => accept.mutate()}
      />
      <BlockersDialog
        open={blockersOpen}
        onOpenChange={setBlockersOpen}
        note={blockersNote}
        reason={overrideReason}
        setReason={setOverrideReason}
        error={overrideError}
        pending={acceptWithOverride.isPending}
        onSubmit={() => {
          if (!overrideReason.trim()) {
            setOverrideError("An override reason is required and is recorded in the audit trail.");
            return;
          }
          setOverrideError(null);
          acceptWithOverride.mutate(overrideReason.trim());
        }}
      />
      <RejectDialog
        open={rejectOpen}
        onOpenChange={setRejectOpen}
        reason={reason}
        setReason={setReason}
        error={reasonError}
        pending={reject.isPending}
        onSubmit={() => {
          if (!reason.trim()) {
            setReasonError("A rejection reason is required.");
            return;
          }
          setReasonError(null);
          reject.mutate();
        }}
      />
    </div>
  );
}

function AcceptDialog({
  open,
  onOpenChange,
  pending,
  assigneeName,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  pending: boolean;
  assigneeName?: string;
  onSubmit: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent role="alertdialog">
        <DialogHeader>
          <DialogTitle>Accept and close this task?</DialogTitle>
          <DialogDescription>
            This will close the task{assigneeName ? ` — ${assigneeName} will be notified` : "; the assignee will be notified"}.
            Closed tasks cannot reopen, but they stay visible in the project history.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={pending}>
            Cancel
          </Button>
          <Button onClick={onSubmit} disabled={pending} data-testid="confirm-accept">
            {pending ? "Closing…" : "Accept & Close"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Close-with-blockers (Phase 14 WS6, ADR-0023): the server returned 409
 * because open `blocks` dependencies exist. Closing anyway requires an
 * explicit override reason, which the server records in the audit trail.
 */
function BlockersDialog({
  open,
  onOpenChange,
  note,
  reason,
  setReason,
  error,
  pending,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  note: string | null;
  reason: string;
  setReason: (v: string) => void;
  error: string | null;
  pending: boolean;
  onSubmit: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent role="alertdialog">
        <DialogHeader>
          <DialogTitle>Open dependencies block this close</DialogTitle>
          <DialogDescription>
            {note ?? "This task is blocked by open dependencies."} Provide a reason to close it
            anyway — the override is recorded in the audit trail.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            onSubmit();
          }}
          noValidate
          data-testid="blockers-override-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="blockers-override-reason">Override reason</Label>
            <Textarea
              id="blockers-override-reason"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="Why is it safe to close this task before its dependencies?"
              rows={3}
              autoFocus
              aria-invalid={Boolean(error)}
              data-testid="blockers-reason-input"
            />
            {error ? (
              <p className="text-2xs text-destructive" role="alert">{error}</p>
            ) : null}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)} disabled={pending}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending} data-testid="blockers-override-submit">
              {pending ? "Closing…" : "Close with override"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function RejectDialog({
  open,
  onOpenChange,
  reason,
  setReason,
  error,
  pending,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  reason: string;
  setReason: (v: string) => void;
  error: string | null;
  pending: boolean;
  onSubmit: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Reject this work?</DialogTitle>
          <DialogDescription>
            The task returns to In Progress and the assignee is notified with your reason.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            onSubmit();
          }}
          noValidate
          data-testid="reject-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="reject-reason">Reason</Label>
            <Textarea
              id="reject-reason"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="What needs to change before this can be accepted?"
              rows={3}
              autoFocus
              aria-invalid={Boolean(error)}
              data-testid="reject-reason-input"
            />
            {error ? (
              <p className="text-2xs text-destructive" role="alert">{error}</p>
            ) : null}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" variant="destructive" disabled={pending} data-testid="reject-submit">
              {pending ? "Rejecting…" : "Reject & send back"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
