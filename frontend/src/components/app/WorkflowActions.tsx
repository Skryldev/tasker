import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Check, Play, SendHorizonal, Undo2 } from "lucide-react";
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
import { Label, Textarea } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";

/**
 * Lifecycle controls for the task detail panel, mirroring the backend state
 * machine exactly:
 *
 *   backlog → todo → in_progress → resolved → in_review → closed
 *                                     ↑__________|  (reject, reason required)
 *
 * The assignee drives backlog/todo/in_progress → resolved; the reporter —
 * never the assignee, even when they hold a privileged role — drives
 * resolved → in_review → closed or back. The buttons render optimistically;
 * the backend remains the authority and surfaces 403/409 as toasts.
 */
export function WorkflowActions({ task }: { task: Task }) {
  const { user } = useAuth();
  const { push } = useToast();
  const qc = useQueryClient();
  const [rejectOpen, setRejectOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [reasonError, setReasonError] = useState<string | null>(null);

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
    onSuccess: refresh,
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

  const isAssignee = task.assignee?.id === user?.id;
  const isReporter = task.reporter?.id === user?.id;

  if (task.status === "in_review") {
    return (
      <div className="flex flex-wrap items-center gap-2" data-testid="workflow-actions">
        {isReporter && !isAssignee ? (
          <>
            <Button size="sm" onClick={() => accept.mutate()} disabled={accept.isPending} data-testid="accept-task">
              <Check /> Accept &amp; Close
            </Button>
            <Button
              size="sm"
              variant="destructive"
              onClick={() => setRejectOpen(true)}
              disabled={reject.isPending}
              data-testid="reject-task"
            >
              <Undo2 /> Reject
            </Button>
          </>
        ) : (
          <p className="text-2xs text-muted-foreground">
            Awaiting review by {task.reporter?.display_name ?? "the reporter"}.
          </p>
        )}
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

  if (task.status === "closed") {
    return null;
  }

  // Assignee: pick up from backlog, start work from todo.
  if (isAssignee && (task.status === "backlog" || task.status === "todo")) {
    const label = task.status === "backlog" ? "Pick up" : "Start work";
    return (
      <Button
        size="sm"
        onClick={() => startProgress.mutate()}
        disabled={startProgress.isPending}
        data-testid="start-task"
      >
        <Play /> {startProgress.isPending ? "…" : label}
      </Button>
    );
  }

  // Assignee: work finished — hand over to the reporter.
  if (task.status === "in_progress" && isAssignee) {
    return (
      <Button size="sm" onClick={() => resolve.mutate()} disabled={resolve.isPending} data-testid="resolve-task">
        <SendHorizonal /> {resolve.isPending ? "Resolving…" : "Mark resolved"}
      </Button>
    );
  }

  // Reporter: submit resolved work for acceptance review.
  if (task.status === "resolved" && isReporter && !isAssignee) {
    return (
      <Button size="sm" onClick={() => submit.mutate()} disabled={submit.isPending} data-testid="submit-task">
        <SendHorizonal /> {submit.isPending ? "Submitting…" : "Submit for review"}
      </Button>
    );
  }

  return null;
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
              {pending ? "Rejecting…" : "Reject task"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
