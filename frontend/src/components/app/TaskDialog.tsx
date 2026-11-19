import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQueryClient } from "@tanstack/react-query";
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
import { Input, Label, Textarea } from "@/components/ui/input";
import { DateField } from "@/components/ui/date-field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast";
import { PRIORITIES, TASK_STATUSES, priorityKey, statusKey, type Priority, type TaskStatus } from "@/lib/utils";
import { PRIORITY_META } from "@/lib/priority-meta";
import { cn } from "@/lib/utils";
import { useLabels, useMembers, useTemplates } from "@/api/hooks";
import { LabelIcon } from "./TaskBadges";
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
  deadline: string;
}

export function TaskDialog(props: TaskDialogProps) {
  const { open, onOpenChange, teamId, projectId, task, parentTaskId } = props;
  const { t } = useTranslation("tasks");
  const { push } = useToast();
  const qc = useQueryClient();
  const { user } = useAuth();
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
    deadline: "",
  });
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [selectedLabels, setSelectedLabels] = useState<string[]>([]);
  // Create-from-template (Phase 11): a picker precedes the blank form for new
  // tasks; selecting one prefills the fields via GET /templates/{id} — every
  // field stays editable and all existing validation applies unchanged.
  const [templateId, setTemplateId] = useState<string>("");
  const templates = useTemplates(projectId);

  useEffect(() => {
    if (open) {
      setFieldErrors({});
      setSelectedLabels(task?.labels.map((l) => l.id) ?? []);
      setTemplateId("");
      setForm({
        title: task?.title ?? "",
        description: task?.description ?? "",
        status: (task?.status as TaskStatus) ?? (parentTaskId ? "todo" : "backlog"),
        priority: (task?.priority as Priority) ?? "medium",
        assignee_id: task?.assignee?.id ?? "none",
        reporter_id: task?.reporter?.id ?? (canPickReporter ? "creator" : "none"),
        due_date: task?.due_date ?? "",
        deadline: task?.deadline ?? "",
      });
    }
  }, [open, task, parentTaskId]);

  const applyTemplate = async (id: string) => {
    const resolved = id === "__blank__" ? "" : id;
    setTemplateId(resolved);
    if (!resolved) {
      setForm((f) => ({ ...f, title: "", description: "", priority: "medium" }));
      setSelectedLabels([]);
      return;
    }
    try {
      const template = await api.getTemplate(id);
      setForm((f) => ({
        ...f,
        title: template.default_title,
        description: template.default_description ?? "",
        priority: (template.default_priority as Priority) ?? "medium",
      }));
      setSelectedLabels(template.label_ids);
    } catch {
      // Template vanished mid-pick: fall back to a blank form.
      setTemplateId("");
    }
  };

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
        deadline: form.deadline || undefined,
        label_ids: selectedLabels.length > 0 ? selectedLabels : undefined,
      };
      if (!task && templateId) {
        // From-template creation (Phase 11): provenance + subtasks + label
        // defaults applied server-side; all overrides stay editable and
        // every validation rule still runs.
        return api.createTaskFromTemplate(templateId, {
          title: body.title,
          default_description: body.description,
          priority: body.priority,
          assignee_id: body.assignee_id,
          due_date: body.due_date,
          status: body.status,
          label_ids: body.label_ids,
        });
      }
      if (task) {
        // Status changes use the lifecycle transition endpoint (board drag or
        // the workflow actions); the generic update only edits fields.
        return api.updateTask(task.id, {
          title: body.title,
          description: form.description,
          priority: body.priority,
          assignee_id: form.assignee_id === "none" ? "" : form.assignee_id,
          reporter_id: canPickReporter
            ? form.reporter_id === "creator" || form.reporter_id === "none"
              ? ""
              : form.reporter_id
            : undefined,
          due_date: form.due_date,
          deadline: form.deadline || null,
          label_ids: selectedLabels,
        });
      }
      if (parentTaskId) {
        return api.createSubtask(parentTaskId, body);
      }
      return api.createTask(projectId, body);
    },
    onSuccess: () => {
      push(
        "success",
        task
          ? t("dialog.toast.updated")
          : parentTaskId
            ? t("dialog.toast.subtaskCreated")
            : t("dialog.toast.created"),
      );
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
        push("error", t("dialog.toast.saveFailed"), error.message);
      }
    },
  });

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.title.trim()) {
      setFieldErrors({ title: t("dialog.errors.titleRequired") });
      return;
    }
    // Phase 12 rule, enforced inline: deadline must be on or after the due
    // date when both are set (the server enforces the same rule).
    if (form.due_date && form.deadline && form.deadline < form.due_date) {
      setFieldErrors({
        deadline: t("dialog.errors.deadlineAfterDue"),
      });
      return;
    }
    mutation.mutate();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent wide>
        <DialogHeader>
          <DialogTitle>
            {task ? t("dialog.title.edit") : parentTaskId ? t("dialog.title.newSubtask") : t("dialog.title.new")}
          </DialogTitle>
          <DialogDescription>
            {task ? t("dialog.desc.edit") : t("dialog.desc.new")}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
          {!task && !parentTaskId ? (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-template">{t("dialog.template.label")}</Label>
              <Select
                value={templateId}
                onValueChange={(v) => void applyTemplate(v)}
              >
                <SelectTrigger id="task-template" data-testid="template-picker">
                  <SelectValue placeholder={t("dialog.template.placeholder")} />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__blank__" data-testid="template-none">
                    {t("dialog.template.startBlank")}
                  </SelectItem>
                  {(templates.data?.items ?? []).map((tpl) => (
                    <SelectItem key={tpl.id} value={tpl.id}>
                      {tpl.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          ) : null}
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="task-title">{t("dialog.fields.title")}</Label>
            <Input
              id="task-title"
              value={form.title}
              onChange={(e) => setForm((f) => ({ ...f, title: e.target.value }))}
              placeholder={t("dialog.fields.titlePlaceholder")}
              autoFocus
              aria-invalid={Boolean(fieldErrors.title)}
              dir="auto"
              data-testid="task-title-input"
            />
            {fieldErrors.title ? (
              <p className="text-2xs text-destructive" role="alert">{fieldErrors.title}</p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="task-description">{t("dialog.fields.description")}</Label>
            <Textarea
              id="task-description"
              value={form.description}
              onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
              placeholder={t("dialog.fields.descriptionPlaceholder")}
              rows={3}
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            {task ? null : (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="task-status">{t("dialog.fields.status")}</Label>
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
                        {t(statusKey(s))}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-priority">{t("dialog.fields.priority")}</Label>
              <Select
                value={form.priority}
                onValueChange={(v) => setForm((f) => ({ ...f, priority: v as Priority }))}
              >
                <SelectTrigger id="task-priority">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PRIORITIES.map((p) => {
                    const meta = PRIORITY_META[p];
                    return (
                      <SelectItem key={p} value={p} aria-describedby={`priority-desc-${p}`}>
                        <span className="flex items-center gap-2">
                          <meta.icon className={cn("h-4 w-4 shrink-0", meta.color)} aria-hidden />
                          <span className="flex flex-col">
                            <span className="text-sm font-semibold leading-tight">
                              {t(priorityKey(p))}
                            </span>
                            <span
                              id={`priority-desc-${p}`}
                              className="text-xs font-normal text-muted-foreground"
                              style={{ lineHeight: 1.3 }}
                            >
                              {meta.fa} — {meta.en}
                            </span>
                          </span>
                        </span>
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-assignee">{t("dialog.fields.assignee")}</Label>
              <Select
                value={form.assignee_id}
                onValueChange={(v) => setForm((f) => ({ ...f, assignee_id: v }))}
              >
                <SelectTrigger id="task-assignee">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">{t("dialog.fields.unassigned")}</SelectItem>
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
                <Label htmlFor="task-reporter">{t("dialog.fields.reporter")}</Label>
                <Select
                  value={form.reporter_id}
                  onValueChange={(v) => setForm((f) => ({ ...f, reporter_id: v }))}
                >
                  <SelectTrigger id="task-reporter">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="creator" data-testid="reporter-you">
                      {user ? t("dialog.fields.reporterYou", { name: user.display_name }) : t("dialog.fields.reporterMe")}
                    </SelectItem>
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
              <Label htmlFor="task-due">{t("dialog.fields.dueDate")}</Label>
              <DateField
                id="task-due"
                value={form.due_date || null}
                onChange={(v) => setForm((f) => ({ ...f, due_date: v }))}
                ariaLabel={t("dialog.fields.dueDate")}
              />
              {fieldErrors.due_date ? (
                <p className="text-2xs text-destructive" role="alert">{fieldErrors.due_date}</p>
              ) : null}
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-deadline">{t("dialog.fields.deadline")}</Label>
              <DateField
                id="task-deadline"
                value={form.deadline || null}
                onChange={(v) => setForm((f) => ({ ...f, deadline: v }))}
                ariaLabel={t("dialog.fields.deadline")}
                placeholder={t("dialog.fields.deadlinePlaceholder")}
              />
              <p className="text-2xs text-muted-foreground">{t("dialog.fields.deadlineHint")}</p>
              {fieldErrors.deadline ? (
                <p className="text-2xs text-destructive" role="alert">{fieldErrors.deadline}</p>
              ) : null}
            </div>
          </div>
          {labels.data && labels.data.items.length > 0 ? (
            <div className="flex flex-col gap-1.5">
              <Label>{t("dialog.fields.labels")}</Label>
              <div className="flex flex-wrap gap-1.5" data-testid="label-picker">
                {labels.data.items.map((label) => {
                  const active = selectedLabels.includes(label.id);
                  // WS5: pastel chips tinted with the label color; the stored
                  // Lucide icon renders when one is set.
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
                      className="inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-2xs font-medium transition-colors"
                      style={
                        active
                          ? { backgroundColor: `${label.color}26`, borderColor: `${label.color}66`, color: "inherit" }
                          : undefined
                      }
                    >
                      {label.icon ? (
                        <LabelIcon icon={label.icon} />
                      ) : (
                        <span className="h-2 w-2 rounded-full" style={{ backgroundColor: label.color }} />
                      )}
                      {label.name}
                    </button>
                  );
                })}
              </div>
            </div>
          ) : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("dialog.cancel")}
            </Button>
            <Button type="submit" disabled={mutation.isPending} data-testid="task-submit">
              {mutation.isPending ? t("dialog.saving") : task ? t("dialog.saveChanges") : t("dialog.createTask")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
