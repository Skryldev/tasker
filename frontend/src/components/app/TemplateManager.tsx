import { useState } from "react";
import { useConfirmDialog } from "@/components/ui/confirm-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { FileStack, Plus, Trash2, Wand2 } from "lucide-react";
import { api, ApiError, type TaskTemplate } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/badge";
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
import { PRIORITIES, PRIORITY_LABELS } from "@/lib/utils";
import { useLabels } from "@/api/hooks";
import { useWorkspace } from "@/lib/workspace";

type Priority = "low" | "medium" | "high" | "urgent";

interface TemplateFormState {
  name: string;
  default_title: string;
  default_description: string;
  default_priority: Priority;
  label_ids: string[];
}

const EMPTY: TemplateFormState = {
  name: "",
  default_title: "",
  default_description: "",
  default_priority: "medium" as Priority,
  label_ids: [],
};

/** Project task template manager (Manager+): reusable defaults for new tasks.
 *  Templates are snapshots — editing one never changes created tasks. */
export function TemplateManager({ projectId }: { projectId: string }) {
  const { myRole } = useWorkspace();
  const labels = useLabels(projectId);
  const qc = useQueryClient();
  const { push } = useToast();
  const canManage = myRole !== undefined && ["owner", "admin", "manager"].includes(myRole);

  const templates = useQuery({
    queryKey: ["templates", projectId],
    queryFn: () => api.listTemplates(projectId),
  });

  const { confirm, element: confirmElement } = useConfirmDialog();
  const [editorOpen, setEditorOpen] = useState(false);
  const [editing, setEditing] = useState<TaskTemplate | null>(null);
  const [form, setForm] = useState<TemplateFormState>(EMPTY);
  const [error, setError] = useState<string | null>(null);

  const invalidate = () => void qc.invalidateQueries({ queryKey: ["templates", projectId] });

  const save = useMutation({
    mutationFn: () => {
      const body = {
        name: form.name,
        default_title: form.default_title,
        default_description: form.default_description || undefined,
        default_priority: form.default_priority,
        label_ids: form.label_ids,
      };
      return editing
        ? api.updateTemplate(editing.id, body)
        : api.createTemplate(projectId, body);
    },
    onSuccess: () => {
      push("success", editing ? "Template updated" : "Template created");
      setEditorOpen(false);
      setEditing(null);
      setForm(EMPTY);
      invalidate();
    },
    onError: (err) =>
      setError(err instanceof ApiError ? err.message : "Could not save the template."),
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteTemplate(id),
    onSuccess: () => {
      push("info", "Template deleted");
      invalidate();
    },
    onError: (err) =>
      push("error", "Could not delete template", err instanceof Error ? err.message : undefined),
  });

  const openEditor = (template: TaskTemplate | null) => {
    setEditing(template);
    setForm(
      template
        ? {
            name: template.name,
            default_title: template.default_title,
            default_description: template.default_description ?? "",
            default_priority: template.default_priority as Priority,
            label_ids: template.label_ids,
          }
        : EMPTY,
    );
    setError(null);
    setEditorOpen(true);
  };

  const submit = () => {
    if (!form.name.trim()) return setError("Template name is required.");
    if (!form.default_title.trim()) return setError("Default title is required.");
    setError(null);
    save.mutate();
  };

  return (
    <section aria-label="Task templates" className="mt-8" data-testid="template-manager">
      {confirmElement}
      <div className="mb-3 flex items-center justify-between">
        <div>
          <h2 className="flex items-center gap-1.5 text-sm font-semibold">
            <FileStack className="h-4 w-4" /> Task templates
          </h2>
          <p className="text-xs text-muted-foreground">
            Reusable defaults for new tasks. Editing a template never changes
            tasks created from it.
          </p>
        </div>
        {canManage ? (
          <Button size="sm" variant="outline" onClick={() => openEditor(null)} data-testid="create-template">
            <Plus /> New template
          </Button>
        ) : null}
      </div>

      {templates.isLoading ? (
        <Skeleton className="h-16 w-full" />
      ) : (templates.data?.items.length ?? 0) === 0 ? (
        <p className="text-xs text-muted-foreground">No templates yet.</p>
      ) : (
        <ul className="flex flex-col gap-2" data-testid="template-list">
          {templates.data!.items.map((template) => (
            <li
              key={template.id}
              className="flex items-center gap-3 rounded-lg border px-3.5 py-2.5"
              data-testid={`template-${template.id}`}
            >
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{template.name}</p>
                <p className="truncate text-2xs text-muted-foreground">
                  {template.default_title} · {PRIORITY_LABELS[
                    template.default_priority as keyof typeof PRIORITY_LABELS
                  ] ?? template.default_priority}
                  {template.default_description ? " · with description" : ""}
                </p>
              </div>
              <TemplateUseButton template={template} />
              {canManage ? (
                <>
                  <Button variant="ghost" size="sm" onClick={() => openEditor(template)}>
                    Edit
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Delete ${template.name}`}
                    className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                    onClick={() =>
                      confirm({
                        title: "Delete this template?",
                        description: `"${template.name}" is permanently removed.`,
                        confirmLabel: "Delete template",
                        action: () => remove.mutate(template.id),
                      })
                    }
                  >
                    <Trash2 />
                  </Button>
                </>
              ) : null}
            </li>
          ))}
        </ul>
      )}

      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editing ? "Edit template" : "New template"}</DialogTitle>
            <DialogDescription>
              Defaults applied when a member starts a task from this template.
              All task validation still applies at creation time.
            </DialogDescription>
          </DialogHeader>
          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
            noValidate
            data-testid="template-form"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="template-name">Template name</Label>
              <Input
                id="template-name"
                value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                placeholder="e.g. Bug report"
                autoFocus
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="template-title">Default task title</Label>
              <Input
                id="template-title"
                value={form.default_title}
                onChange={(e) => setForm((f) => ({ ...f, default_title: e.target.value }))}
                placeholder="[Bug] "
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="template-description">Default description</Label>
              <Textarea
                id="template-description"
                value={form.default_description}
                onChange={(e) => setForm((f) => ({ ...f, default_description: e.target.value }))}
                placeholder="Pre-filled description (editable per task)"
                rows={3}
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="template-priority">Default priority</Label>
                <Select
                  value={form.default_priority}
                  onValueChange={(v) =>
                    setForm((f) => ({ ...f, default_priority: v as Priority }))
                  }
                >
                  <SelectTrigger id="template-priority">
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
                <Label>Default labels</Label>
                <div className="flex flex-wrap gap-1.5" data-testid="template-label-picker">
                  {(labels.data?.items ?? []).map((label) => {
                    const active = form.label_ids.includes(label.id);
                    return (
                      <button
                        key={label.id}
                        type="button"
                        aria-pressed={active}
                        onClick={() =>
                          setForm((f) => ({
                            ...f,
                            label_ids: active
                              ? f.label_ids.filter((id) => id !== label.id)
                              : [...f.label_ids, label.id],
                          }))
                        }
                        className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-2xs ${
                          active
                            ? "border-primary bg-primary/10 text-primary"
                            : "text-muted-foreground hover:bg-accent"
                        }`}
                      >
                        <span
                          className="h-2 w-2 rounded-full"
                          style={{ backgroundColor: label.color }}
                        />
                        {label.name}
                      </button>
                    );
                  })}
                </div>
              </div>
            </div>
            {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
            <DialogFooter>
              <Button variant="outline" onClick={() => setEditorOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={save.isPending} data-testid="template-submit">
                {save.isPending ? "Saving…" : editing ? "Save changes" : "Create template"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </section>
  );
}

/** Opens the standard TaskDialog pre-filled from the template. The created
 *  task is a fully independent, normally validated task. */
function TemplateUseButton({ template }: { template: TaskTemplate }) {
  const { push } = useToast();
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);

  const create = useMutation({
    mutationFn: (values: { title: string; description: string; priority: string; label_ids: string[] }) =>
      api.createTask(template.project_id, {
        title: values.title,
        description: values.description || undefined,
        priority: values.priority as "low" | "medium" | "high" | "urgent",
        label_ids: values.label_ids.length > 0 ? values.label_ids : undefined,
      }),
    onSuccess: () => {
      push("success", "Task created from template");
      void qc.invalidateQueries({ queryKey: ["tasks"] });
      void qc.invalidateQueries({ queryKey: ["board"] });
      void qc.invalidateQueries({ queryKey: ["my-tasks"] });
      setOpen(false);
    },
    onError: (err) =>
      push("error", "Could not create task", err instanceof ApiError ? err.message : undefined),
  });

  const [title, setTitle] = useState(template.default_title);
  const [description, setDescription] = useState(template.default_description ?? "");
  const [priority, setPriority] = useState<Priority>(
    template.default_priority as Priority,
  );

  return (
    <>
      <Button variant="ghost" size="sm" onClick={() => setOpen(true)} data-testid={`use-template-${template.id}`}>
        <Wand2 /> Use
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New task from “{template.name}”</DialogTitle>
            <DialogDescription>
              Pre-filled from the template — edit freely before creating.
            </DialogDescription>
          </DialogHeader>
          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (!title.trim()) return;
              create.mutate({ title, description, priority, label_ids: template.label_ids });
            }}
            noValidate
            data-testid="use-template-form"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="use-title">Title</Label>
              <Input id="use-title" value={title} onChange={(e) => setTitle(e.target.value)} />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="use-description">Description</Label>
              <Textarea
                id="use-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={4}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="use-priority">Priority</Label>
              <Select
                value={priority}
                onValueChange={(v) => setPriority(v as Priority)}
              >
                <SelectTrigger id="use-priority">
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
            {template.label_ids.length > 0 ? (
              <p className="text-2xs text-muted-foreground">
                {template.label_ids.length} label{template.label_ids.length === 1 ? "" : "s"} will be
                attached.
              </p>
            ) : null}
            <DialogFooter>
              <Button variant="outline" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={create.isPending || !title.trim()} data-testid="use-template-submit">
                {create.isPending ? "Creating…" : "Create task"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
