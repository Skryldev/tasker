import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Check, SendHorizonal, Undo2 } from "lucide-react";
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
import { useWorkspace } from "@/lib/workspace";

/**
 * Submit / Accept / Reject controls for the task detail panel.
 *
 * Permissions mirror the backend: the assignee (or creator/manager) submits;
 * the reporter or an Owner/Admin accepts or rejects — never the assignee,
 * even when they are the reporter. The buttons are rendered optimistically;
 * the backend remains the authority and surfaces 403/409 as toasts.
 */
export function WorkflowActions({ task }: { task: Task }) {
  const { user } = useAuth();
  const { myRole } = useWorkspace();
  const isManager = myRole !== undefined && ["owner", "admin", "manager"].includes(myRole);
  const canReviewOverride = myRole !== undefined && ["owner", "admin"].includes(myRole);
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

  const submit = useMutation({
    mutationFn: () => api.submitForReview(task.id),
    onSuccess: refresh,
    onError,
  });
  const accept = useMutation({
    mutationFn: () => api.acceptTask(task.id),
    onSuccess: refresh,
    onError,
  });
  const reject = useMutation({
    mutationFn: () => api.rejectTask(task.id, reason.trim()),
    onSuccess: (updated) => {
      setRejectOpen(false);
      setReason("");
      refresh(updated);
    },
    onError,
  });

  const isAssignee = task.assignee?.id === user?.id;
  void task.reporter;

  if (task.status === "in_review") {
    return (
      <div className="flex flex-wrap items-center gap-2" data-testid="workflow-actions">
        {canReviewOverride ? (
          <>
            <Button size="sm" onClick={() => accept.mutate()} disabled={accept.isPending} data-testid="accept-task">
              <Check /> Accept
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

  if (task.status === "done") {
    return null;
  }

  // Submit is offered to the assignee (or creator/manager) from in_progress.
  const canSubmit =
    task.status === "in_progress" &&
    (isAssignee || task.created_by?.id === user?.id || isManager);

  if (!canSubmit) {
    return null;
  }

  return (
    <Button size="sm" onClick={() => submit.mutate()} disabled={submit.isPending} data-testid="submit-task">
      <SendHorizonal /> {submit.isPending ? "Submitting…" : "Submit for review"}
    </Button>
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
              {pending ? "Rejecting…" : "Reject task"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
