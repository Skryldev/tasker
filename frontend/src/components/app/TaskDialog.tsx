import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, ApiError, type Task } from "@/api/client";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast";
import { PRIORITIES, PRIORITY_LABELS, TASK_STATUSES, STATUS_LABELS, type Priority, type TaskStatus } from "@/lib/utils";
import { useLabels, useMembers } from "@/api/hooks";
import { useWorkspace } from "@/lib/workspace";

export interface TaskDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  teamId: string;
  projectId: string;
  /** When set the dialog edits this task; otherwise it creates a new one. */
  task?: Task;
  /** When set the dialog creates a subtask of this parent. */
  parentTaskId?: string;
}

interface FormState {
  title: string;
  description: string;
  status: TaskStatus;
  priority: Priority;
  assignee_id: string;
  reporter_id: string;
  due_date: string;
}

export function TaskDialog(props: TaskDialogProps) {
  const { open, onOpenChange, teamId, projectId, task, parentTaskId } = props;
  const { push } = useToast();
  const qc = useQueryClient();
  const members = useMembers(teamId);
  const labels = useLabels(teamId);
  const { myRole } = useWorkspace();
  const canPickReporter =
    myRole !== undefined && ["owner", "admin", "manager"].includes(myRole);
  // New tasks always start in an actionable state; review states are reached
  // through the workflow operations.
  const selectableStatuses = task
    ? TASK_STATUSES
    : (["backlog", "todo", "in_progress"] as const);
  const [form, setForm] = useState<FormState>({
    title: "",
    description: "",
    status: "backlog",
    priority: "medium",
    assignee_id: "none",
    reporter_id: "creator",
    due_date: "",
  });
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [selectedLabels, setSelectedLabels] = useState<string[]>([]);

  useEffect(() => {
    if (open) {
      setFieldErrors({});
      setSelectedLabels(task?.labels.map((l) => l.id) ?? []);
      setForm({
        title: task?.title ?? "",
        description: task?.description ?? "",
        status: (task?.status as TaskStatus) ?? (parentTaskId ? "todo" : "backlog"),
        priority: (task?.priority as Priority) ?? "medium",
        assignee_id: task?.assignee?.id ?? "none",
        reporter_id: task?.reporter?.id ?? (canPickReporter ? "creator" : "none"),
        due_date: task?.due_date ?? "",
      });
    }
  }, [open, task, parentTaskId]);

  const mutation = useMutation({
    mutationFn: async () => {
      const reporter =
        canPickReporter && form.reporter_id !== "creator" && form.reporter_id !== "none"
          ? form.reporter_id
          : undefined;
      const CREATION_STATUSES = ["backlog", "todo", "in_progress"] as const;
      type CreationStatus = (typeof CREATION_STATUSES)[number];
      const creationStatus: CreationStatus = CREATION_STATUSES.includes(
        form.status as CreationStatus,
      )
        ? (form.status as CreationStatus)
        : "todo";
      const body = {
        title: form.title,
        description: form.description || undefined,
        status: creationStatus,
        priority: form.priority,
        assignee_id: form.assignee_id === "none" ? undefined : form.assignee_id,
        reporter_id: reporter,
        due_date: form.due_date || undefined,
        label_ids: selectedLabels.length > 0 ? selectedLabels : undefined,
      };
      if (task) {
        return api.updateTask(task.id, {
          title: body.title,
          description: form.description,
          status: body.status,
          priority: body.priority,
          assignee_id: form.assignee_id === "none" ? "" : form.assignee_id,
          reporter_id: canPickReporter
            ? form.reporter_id === "creator" || form.reporter_id === "none"
              ? ""
              : form.reporter_id
            : undefined,
          due_date: form.due_date,
          label_ids: selectedLabels,
        });
      }
      if (parentTaskId) {
        return api.createSubtask(parentTaskId, body);
      }
      return api.createTask(projectId, body);
    },
    onSuccess: () => {
      push("success", task ? "Task updated" : parentTaskId ? "Subtask created" : "Task created");
      void qc.invalidateQueries({ queryKey: ["task"] });
      void qc.invalidateQueries({ queryKey: ["board"] });
      void qc.invalidateQueries({ queryKey: ["tasks"] });
      void qc.invalidateQueries({ queryKey: ["my-tasks"] });
      void qc.invalidateQueries({ queryKey: ["projects"] });
      onOpenChange(false);
    },
    onError: (error) => {
      if (error instanceof ApiError && error.problem.errors) {
        const errors: Record<string, string> = {};
        for (const e of error.problem.errors) errors[e.field] = e.message;
        setFieldErrors(errors);
      } else {
        push("error", "Could not save task", error.message);
      }
    },
  });

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.title.trim()) {
      setFieldErrors({ title: "Title is required" });
      return;
    }
    mutation.mutate();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent wide>
        <DialogHeader>
          <DialogTitle>
            {task ? "Edit task" : parentTaskId ? "New subtask" : "New task"}
          </DialogTitle>
          <DialogDescription>
            {task ? "Changes are recorded in the task history." : "Fill in the task details."}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="task-title">Title</Label>
            <Input
              id="task-title"
              value={form.title}
              onChange={(e) => setForm((f) => ({ ...f, title: e.target.value }))}
              placeholder="Short, actionable summary"
              autoFocus
              aria-invalid={Boolean(fieldErrors.title)}
              data-testid="task-title-input"
            />
            {fieldErrors.title ? (
              <p className="text-2xs text-destructive" role="alert">{fieldErrors.title}</p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="task-description">Description</Label>
            <Textarea
              id="task-description"
              value={form.description}
              onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
              placeholder="Context, links, acceptance criteria…"
              rows={3}
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-status">Status</Label>
              <Select
                value={form.status}
                onValueChange={(v) => setForm((f) => ({ ...f, status: v as TaskStatus }))}
              >
                <SelectTrigger id="task-status">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {selectableStatuses.map((s) => (
                    <SelectItem key={s} value={s}>
                      {STATUS_LABELS[s]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-priority">Priority</Label>
              <Select
                value={form.priority}
                onValueChange={(v) => setForm((f) => ({ ...f, priority: v as Priority }))}
              >
                <SelectTrigger id="task-priority">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PRIORITIES.map((p) => (
                    <SelectItem key={p} value={p}>
                      {PRIORITY_LABELS[p]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-assignee">Assignee</Label>
              <Select
                value={form.assignee_id}
                onValueChange={(v) => setForm((f) => ({ ...f, assignee_id: v }))}
              >
                <SelectTrigger id="task-assignee">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">Unassigned</SelectItem>
                  {members.data?.items.map((m) => (
                    <SelectItem key={m.user.id} value={m.user.id}>
                      {m.user.display_name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {fieldErrors.assignee_id ? (
                <p className="text-2xs text-destructive" role="alert">{fieldErrors.assignee_id}</p>
              ) : null}
            </div>
            {canPickReporter ? (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="task-reporter">Reporter</Label>
                <Select
                  value={form.reporter_id}
                  onValueChange={(v) => setForm((f) => ({ ...f, reporter_id: v }))}
                >
                  <SelectTrigger id="task-reporter">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="creator">Me (creator)</SelectItem>
                    {members.data?.items.map((m) => (
                      <SelectItem key={m.user.id} value={m.user.id}>
                        {m.user.display_name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {fieldErrors.reporter_id ? (
                  <p className="text-2xs text-destructive" role="alert">{fieldErrors.reporter_id}</p>
                ) : null}
              </div>
            ) : null}
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-due">Due date</Label>
              <Input
                id="task-due"
                type="date"
                value={form.due_date}
                onChange={(e) => setForm((f) => ({ ...f, due_date: e.target.value }))}
              />
              {fieldErrors.due_date ? (
                <p className="text-2xs text-destructive" role="alert">{fieldErrors.due_date}</p>
              ) : null}
            </div>
          </div>
          {labels.data && labels.data.items.length > 0 ? (
            <div className="flex flex-col gap-1.5">
              <Label>Labels</Label>
              <div className="flex flex-wrap gap-1.5" data-testid="label-picker">
                {labels.data.items.map((label) => {
                  const active = selectedLabels.includes(label.id);
                  return (
                    <button
                      key={label.id}
                      type="button"
                      aria-pressed={active}
                      onClick={() =>
                        setSelectedLabels((prev) =>
                          active ? prev.filter((id) => id !== label.id) : [...prev, label.id],
                        )
                      }
                      className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-2xs font-medium transition-colors ${
                        active ? "border-primary bg-primary/10 text-primary" : "text-muted-foreground hover:bg-accent"
                      }`}
                    >
                      <span className="h-2 w-2 rounded-full" style={{ backgroundColor: label.color }} />
                      {label.name}
                    </button>
                  );
                })}
              </div>
            </div>
          ) : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={mutation.isPending} data-testid="task-submit">
              {mutation.isPending ? "Saving…" : task ? "Save changes" : "Create task"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
