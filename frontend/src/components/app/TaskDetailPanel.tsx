import { lazy, Suspense, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  CalendarDays,
  Check,
  ChevronDown,
  Flag,
  History,
  Link2,
  Lock,
  Maximize2,
  MessageSquare,
  MoreHorizontal,
  Paperclip,
  Pencil,
  Plus,
  Stamp,
  ListTree,
  Trash2,
  X,
} from "lucide-react";
import { api, type Task } from "@/api/client";
import { avatarUrl } from "@/lib/avatar-version";
import { useTemplates } from "@/api/hooks";
import { cn, PRIORITY_LABELS, PRIORITIES, type Priority } from "@/lib/utils";
import {
  useActivity,
  useAttachments,
  useChecklist,
  useComments,
  useCreateChecklistItem,
  useCreateComment,
  useDeleteAttachment,
  useDeleteChecklistItem,
  useDeleteComment,
  useDeleteTask,
  useLabels,
  useMembers,
  useTaskProgress,
  useUpdateChecklistItem,
  useUpdateComment,
  useUpdateTask,
  useUploadAttachment,
  useUserInfo,
} from "@/api/hooks";
import { Avatar, Separator } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/badge";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { LabelIcon, StatusBadge } from "./TaskBadges";
import { LabelChip } from "./LabelChip";
// Phase 10: the edit dialog pulls the Doran calendar stack; it is loaded on
// first open so the eager task views do not carry it.
const TaskDialog = lazy(() => import("./TaskDialog").then((m) => ({ default: m.TaskDialog })));
// Phase 11: the markdown editor (toolbar + slash commands) is pulled in with
// the first description/comment edit.
const MarkdownEditor = lazy(() =>
  import("./MarkdownEditor").then((m) => ({ default: m.MarkdownEditor })),
);
import { StatusControl } from "./StatusControl";
import { CommentComposer } from "./Mentions";
import { CommentThread, buildThreads } from "./CommentThread";
import { DependenciesPanel } from "./DependenciesPanel";
// Lazy: the Doran picker stack (~56 kB raw) must stay out of the eager
// bundle (Phase 10 gate). It loads on the first due-date inline edit.
const DateField = lazy(() =>
  import("@/components/ui/date-field").then((m) => ({ default: m.DateField })),
);
import { useToast } from "@/components/ui/toast";

import { Timestamp } from "@/components/app/Timestamp";
import { formatJalaliDate } from "@/lib/jalali";
import { renderMarkdown } from "@/lib/markdown";
import { useQueryClient } from "@tanstack/react-query";

const EVENT_LABELS: Record<string, string> = {
  task_created: "created this task",
  task_updated: "updated this task",
  task_status_changed: "changed status",
  task_priority_changed: "changed priority",
  task_assignee_changed: "changed assignee",
  task_due_date_changed: "changed due date",
  comment_added: "commented",
  comment_edited: "edited a comment",
  comment_deleted: "deleted a comment",
};

function eventDetail(field: string | null, oldValue: string | null | undefined, newValue: string | null | undefined): string {
  if (!field) return newValue ?? "";
  const fmt = (v: string | null | undefined) => v ?? "none";
  return `${field.replaceAll("_", " ")}: ${fmt(oldValue)} → ${fmt(newValue)}`;
}

export function TaskDetailPanel({
  task,
  onClose,
  hidden,
  onOpenFullPage,
}: {
  task: Task | undefined;
  onClose: () => void;
  /** Bug 3: when the full page is open the panel stays MOUNTED but out of
      view and out of the tab order — state survives the round trip. */
  hidden?: boolean;
  /** Bug 1: navigate to the full page (the panel does not own the route). */
  onOpenFullPage?: () => void;
}) {
  const [editOpen, setEditOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const navigate = useNavigate();
  const { push } = useToast();
  const qc = useQueryClient();
  // Phase 12 WS10: closed tasks are immutable; the panel switches to a
  // read-only presentation (no field chips edits, no editor, no composer).
  const isClosed = task?.status === "closed";
  const deleteTask = useDeleteTask();
  const updateTaskBody = useUpdateTaskBody();
  // Bug 1: first-visit discoverability — the expand affordance pulses once
  // (150ms), never again (localStorage flag). Respects reduced motion via
  // the Phase 10 global rule (animations disabled there).
  const [pulseOnce, setPulseOnce] = useState(
    () => localStorage.getItem("tasker.fullPageHintShown") !== "1",
  );
  const markHintShown = () => {
    if (pulseOnce) {
      localStorage.setItem("tasker.fullPageHintShown", "1");
      setPulseOnce(false);
    }
  };
  const openFullPage = () => {
    markHintShown();
    if (task) {
      if (onOpenFullPage) onOpenFullPage();
      else navigate(`/teams/${task.team_id}/projects/${task.project_id}/tasks/${task.id}`);
    }
  };

  if (!task) {
    return (
      <div className="flex w-full flex-col items-center justify-center gap-3 p-10 text-sm text-muted-foreground">
        <History className="h-8 w-8 opacity-40" />
        Select a task to see its details.
      </div>
    );
  }

  const confirmDeleteTask = () => {
    deleteTask.mutate(task.id, {
      onSuccess: () => {
        push("info", "Task deleted");
        setDeleteOpen(false);
        onClose();
      },
      onError: (error) =>
        push("error", "Could not delete task", error instanceof Error ? error.message : undefined),
    });
  };

  const copyLink = () => {
    const url = `${window.location.origin}/teams/${task.team_id}/projects/${task.project_id}/tasks/${task.id}`;
    void navigator.clipboard
      ?.writeText(url)
      .then(() => push("success", "Link copied"))
      .catch(() => push("error", "Could not copy the link"));
  };

  return (
    <div
      className={cn("flex h-full flex-col", hidden && "hidden")}
      data-testid="task-detail"
      aria-hidden={hidden || undefined}
    >
      {/* Sticky header: identity, title, overflow actions. */}
      <header className="shrink-0 border-b p-4">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <p className="flex items-center gap-1.5 font-mono text-2xs text-muted-foreground">
              Task · {task.id.slice(0, 8)}
              {isClosed ? (
                <span
                  className="inline-flex items-center gap-1 rounded-full border border-muted-foreground/30 bg-muted px-1.5 py-0.5 font-sans text-2xs font-medium"
                  data-testid="task-locked"
                >
                  <Lock className="h-2.5 w-2.5" aria-hidden /> Locked
                </span>
              ) : null}
            </p>
            <h2 className="mt-0.5 line-clamp-3 text-lg font-semibold leading-snug">
              {task.title}
            </h2>
          </div>
          <div className="flex items-center gap-0.5">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Open task as full page"
              title="Open full page (⌘⇧O)"
              onClick={openFullPage}
              data-testid="expand-full-page"
              className={pulseOnce ? "animate-[tasker-pulse_150ms_ease-out_2]" : undefined}
            >
              <Maximize2 />
            </Button>
            <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-sm" aria-label="Task actions" data-testid="task-actions">
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-44">
              <DropdownMenuItem onSelect={openFullPage} data-testid="open-full-page">
                <Maximize2 /> Open full page
              </DropdownMenuItem>
              {isClosed ? null : (
                <DropdownMenuItem onSelect={() => setEditOpen(true)} data-testid="edit-task">
                  <Pencil /> Edit fields…
                </DropdownMenuItem>
              )}
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

      <div className="min-h-0 flex-1 overflow-y-auto scrollbar-thin p-4">
        <MetadataBand
          task={task}
          readOnly={isClosed}
          onChange={(body, label) => updateTaskBody(task.id, body, label)}
        />
        <TaskDetailBody
          task={task}
          readOnly={isClosed}
          onChange={(body, label) => updateTaskBody(task.id, body, label)}
        />
      </div>

      {/* Sticky footer: the status control is always within reach. */}
      <footer className="shrink-0 border-t bg-card p-3" data-testid="task-detail-footer">
        <StatusControl task={task} />
      </footer>

      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title="Delete this task?"
        description={`"${task.title}" and its subtasks are permanently removed.`}
        confirmLabel="Delete task"
        pending={deleteTask.isPending}
        onConfirm={confirmDeleteTask}
      />

      {editOpen ? (
        <Suspense fallback={null}>
          <TaskDialogResolver
            open={editOpen}
            onOpenChange={setEditOpen}
            taskId={task.id}
            onSaved={async () => {
              const fresh = await api.getTask(task.id);
              void qc.setQueryData(["task", task.id], fresh);
            }}
          />
        </Suspense>
      ) : null}
    </div>
  );
}

/**
 * Bug 4: the task update helper shared by every surface. Mutates the task
 * and reflects the result into the task cache with a toast.
 */
export function useUpdateTaskBody() {
  const updateTask = useUpdateTask();
  const { push } = useToast();
  const qc = useQueryClient();
  return (taskId: string, body: Record<string, unknown>, label?: string) =>
    updateTask.mutate(
      { taskId, body },
      {
        onSuccess: (updated) => {
          void qc.setQueryData(["task", updated.id], updated);
          if (label) push("success", label);
        },
        onError: (error) =>
          push("error", "Could not update task", error instanceof Error ? error.message : undefined),
      },
    );
}

/**
 * Bug 4: the shared detail body — description, resolution, subtasks,
 * dependencies, checklist, attachments, comments, activity. Both the
 * quick-view panel and the full task page render exactly this component, so
 * behavior and markup cannot drift between the two surfaces.
 */
export function TaskDetailBody({
  task,
  readOnly,
  onChange,
}: {
  task: Task;
  readOnly: boolean;
  /** Shared field-update helper (metadata chips live outside the body). */
  onChange: (body: Record<string, unknown>, label?: string) => void;
}) {
  const [commentDraft, setCommentDraft] = useState("");
  const [editingComment, setEditingComment] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  // Parent-with-replies delete confirmation (Phase 11 threading).
  const [deleteReplies, setDeleteReplies] = useState<{ id: string; count: number } | null>(null);
  const { push } = useToast();

  const taskId = task.id;
  const comments = useComments(taskId);
  const activity = useActivity(taskId);
  const createComment = useCreateComment(taskId);
  const updateComment = useUpdateComment(taskId);
  const deleteComment = useDeleteComment(taskId);
  // Phase 12 WS4/WS11: checklist, progress, and attachments ride along with
  // the body; every mutation invalidates its own slice.
  const checklist = useChecklist(taskId);
  const progress = useTaskProgress(taskId);
  const attachments = useAttachments(taskId);
  const createItem = useCreateChecklistItem(taskId);
  const updateItem = useUpdateChecklistItem(taskId);
  const deleteItem = useDeleteChecklistItem(taskId);
  const uploadAttachment = useUploadAttachment(taskId);
  const deleteAttachment = useDeleteAttachment(taskId);

  // Hover-card identities (username/bio) for every avatar in the body,
  // resolved with one batched request.
  const hoverIds = useMemo(() => {
    const ids = [
      task.assignee?.id,
      task.reporter?.id,
      task.resolved_by?.id,
      ...(comments.data?.items.map((c) => c.author.id) ?? []),
      ...(activity.data?.items.map((a) => a.actor?.id) ?? []),
    ];
    return [...new Set(ids.filter((id): id is string => Boolean(id)))];
  }, [task, comments.data, activity.data]);
  const hoverInfo = useUserInfo(hoverIds);
  const templates = useTemplates(task.project_id);
  const templateName = (id: string) =>
    templates.data?.items.find((t) => t.id === id)?.name ?? "template";
  const hoverTitle = (id: string | undefined, name: string): string | undefined => {
    if (!id) return undefined;
    const info = hoverInfo.data?.items.find((i) => i.id === id);
    if (!info) return undefined;
    return [name, info.username ? `@${info.username}` : null, info.bio ?? null]
      .filter(Boolean)
      .join("\n");
  };

  const submitComment = async () => {
    if (!commentDraft.trim() || createComment.isPending) return;
    try {
      await createComment.mutateAsync({ body: commentDraft.trim() });
      setCommentDraft("");
    } catch (error) {
      push("error", "Could not add comment", error instanceof Error ? error.message : undefined);
    }
  };

  return (
    <>
      {task.from_template_id ? (
        <p
          className="mt-2 inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-2xs text-muted-foreground"
          data-testid="from-template-badge"
        >
          <Stamp className="h-3 w-3" aria-hidden />
          From template: {templateName(task.from_template_id)}
        </p>
      ) : null}

      <DescriptionBlock
        value={task.description ?? ""}
        readOnly={readOnly}
        onCommit={(description) => onChange({ description })}
      />

      {task.resolution ? (
        <div
          className={cn(
            "mt-3 rounded-md border px-3 py-2 text-xs",
            task.resolution === "accepted"
              ? "border-success/30 bg-success/5 text-success"
              : "border-destructive/30 bg-destructive/5 text-destructive",
          )}
          data-testid="resolution-banner"
        >
          {task.resolution === "accepted" ? (
            <>
              Accepted{task.resolved_by ? ` by ${task.resolved_by.display_name}` : ""}
              {task.resolved_at ? (
                <> · <Timestamp value={task.resolved_at} /></>
              ) : (
                ""
              )}
            </>
          ) : (
            <>
              Rejected
              {task.resolution_note ? ` — "${task.resolution_note}"` : ""}
            </>
          )}
        </div>
      ) : null}

      {task.subtasks && task.subtasks.length > 0 ? (
        <section className="mt-5" aria-label="Subtasks">
          <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
            <ListTree className="h-3.5 w-3.5" /> Subtasks ({task.subtasks.length})
          </h3>
          <ul className="flex flex-col gap-1">
            {task.subtasks.map((sub) => (
              <li key={sub.id} className="flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-sm">
                <StatusBadge status={sub.status} />
                <span className={cn("truncate", sub.status === "closed" && "text-muted-foreground line-through")}>
                  {sub.title}
                </span>
                <PriorityBadgeInline priority={sub.priority} className="ml-auto" />
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <DependenciesPanel task={task} />

      <ChecklistSection
        readOnly={readOnly}
        checklist={checklist}
        progress={progress.data}
        onCreate={(title) => createItem.mutate(title, {
          onError: (error) =>
            push("error", "Could not add checklist item", error instanceof Error ? error.message : undefined),
        })}
        onToggle={(itemId, checked) => updateItem.mutate({ itemId, body: { is_checked: checked } }, {
          onError: (error) =>
            push("error", "Could not update checklist item", error instanceof Error ? error.message : undefined),
        })}
        onDelete={(itemId) => deleteItem.mutate(itemId, {
          onError: (error) =>
            push("error", "Could not delete checklist item", error instanceof Error ? error.message : undefined),
        })}
      />

      <AttachmentsSection
        readOnly={readOnly}
        attachments={attachments}
        onUpload={(file) => uploadAttachment.mutate(file, {
          onSuccess: () => push("success", "File attached"),
          onError: (error: unknown) =>
            push("error", "Could not upload file", error instanceof Error ? error.message : undefined),
        })}
        onDelete={(attachmentId) => deleteAttachment.mutate(attachmentId, {
          onError: (error) =>
            push("error", "Could not delete attachment", error instanceof Error ? error.message : undefined),
        })}
      />

      <Separator className="my-5" />

      <section aria-label="Comments" className="mt-1" id="task-comments">
        <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
          <MessageSquare className="h-3.5 w-3.5" /> Comments ({comments.data?.total ?? 0})
        </h3>
        {readOnly ? (
          <p
            className="mb-4 flex items-center gap-1.5 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
            data-testid="comments-locked"
          >
            <Lock className="h-3 w-3" aria-hidden />
            This task is closed; commenting is disabled.
          </p>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void submitComment();
            }}
            className="mb-4"
          >
            <CommentComposer
              teamId={task.team_id}
              value={commentDraft}
              onChange={setCommentDraft}
              onSubmit={() => void submitComment()}
              pending={createComment.isPending}
            />
          </form>
        )}
        <div className="flex flex-col" data-testid="comment-threads">
          {comments.isLoading ? (
            <>
              <Skeleton className="h-14 w-full" />
              <Skeleton className="h-14 w-full" />
            </>
          ) : (
            buildThreads(comments.data?.items ?? []).map((thread) => (
              <CommentThread
                key={thread.parent.id}
                thread={thread}
                task={task}
                hoverTitle={hoverTitle}
                onReply={(body) =>
                  createComment
                    .mutateAsync({ body, parentId: thread.parent.id })
                    .then(() => push("success", "Reply posted"))
                    .catch((error) =>
                      push("error", "Could not reply", error instanceof Error ? error.message : undefined),
                    )
                }
                replyPending={createComment.isPending}
                onEdit={(commentId, body) =>
                  updateComment
                    .mutateAsync({ commentId, body })
                    .then(() => setEditingComment(null))
                    .catch((error) =>
                      push("error", "Could not update comment", error instanceof Error ? error.message : undefined),
                    )
                }
                editingComment={editingComment}
                setEditingComment={setEditingComment}
                editDraft={editDraft}
                setEditDraft={setEditDraft}
                onDelete={(comment) => {
                  const replyCount = comment.id === thread.parent.id ? thread.replies.length : 0;
                  if (replyCount > 0) {
                    setDeleteReplies({ id: comment.id, count: replyCount });
                  } else {
                    deleteComment.mutate(comment.id, {
                      onError: (error) =>
                        push("error", "Could not delete comment", error instanceof Error ? error.message : undefined),
                    });
                  }
                }}
              />
            ))
          )}
        </div>
      </section>

      <Separator className="my-5" />

      <ActivitySection activity={activity.data?.items ?? []} isLoading={activity.isLoading} hoverTitle={hoverTitle} />

      <ConfirmDialog
        open={deleteReplies !== null}
        onOpenChange={(v) => {
          if (!v) setDeleteReplies(null);
        }}
        title={`Delete this comment and its ${deleteReplies?.count ?? 0} repl${deleteReplies?.count === 1 ? "y" : "ies"}?`}
        description="Deleting the parent removes its replies too. This cannot be undone."
        confirmLabel="Delete all"
        pending={deleteComment.isPending}
        onConfirm={() => {
          if (deleteReplies) {
            deleteComment.mutate(deleteReplies.id, {
              onSuccess: () => setDeleteReplies(null),
              onError: (error) =>
                push("error", "Could not delete comment", error instanceof Error ? error.message : undefined),
            });
          }
        }}
      />
    </>
  );
}


/** The edit dialog needs the project context; refetch it from the task. */
function TaskDialogResolver({
  open,
  onOpenChange,
  taskId,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  taskId: string;
  onSaved: () => Promise<unknown>;
}) {
  const qc = useQueryClient();
  const task = qc.getQueryData<Task>(["task", taskId]);
  if (!task) return null;
  return (
    <TaskDialog
      open={open}
      onOpenChange={(v) => {
        if (!v) void onSaved();
        onOpenChange(v);
      }}
      teamId={task.team_id}
      projectId={task.project_id}
      task={task}
    />
  );
}

// ---------------------------------------------------------------------------
// Metadata band: reporter / assignee / priority / due / labels, each a chip
// with click-to-edit (dropdowns inline; no modal for simple fields).
// ---------------------------------------------------------------------------

export function MetadataBand({
  task,
  readOnly,
  onChange,
}: {
  task: Task;
  /** WS10: closed tasks render chips as static text, no editors. */
  readOnly: boolean;
  onChange: (body: Record<string, unknown>, label: string) => void;
}) {
  const members = useMembers(task.team_id);
  const labels = useLabels(task.team_id);
  const [dueEditing, setDueEditing] = useState(false);
  const [deadlineEditing, setDeadlineEditing] = useState(false);
  const [labelsOpen, setLabelsOpen] = useState(false);

  const chipClass =
    "inline-flex max-w-[14rem] items-center gap-1.5 rounded-full border px-2 py-1 text-2xs transition-colors duration-100 hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
  const staticChipClass =
    "inline-flex max-w-[14rem] items-center gap-1.5 rounded-full border px-2 py-1 text-2xs";

  const selectedLabelIds = new Set(task.labels.map((l) => l.id));
  const toggleLabel = (id: string) => {
    const next = task.labels.some((l) => l.id === id)
      ? task.labels.filter((l) => l.id !== id).map((l) => l.id)
      : [...task.labels.map((l) => l.id), id];
    onChange({ label_ids: next }, "Labels updated");
  };

  // WS3: the deadline chip is red-tinted and flags the hard cutoff; when the
  // deadline has passed it turns fully destructive.
  const today = new Date().toISOString().slice(0, 10);
  const deadlinePassed =
    Boolean(task.deadline && task.due_date && task.deadline < task.due_date) ||
    Boolean(task.deadline && task.deadline < today);
  const deadlineChipClass = cn(
    readOnly ? staticChipClass : chipClass,
    task.deadline &&
      "border-destructive/40 bg-destructive/10 text-destructive hover:bg-destructive/15",
    deadlinePassed && "border-destructive/60 bg-destructive/20",
  );

  return (
    <div className="flex flex-wrap items-center gap-1.5" id="task-dates" data-testid="task-metadata">
      {/* Reporter */}
      {readOnly ? (
        <span className={staticChipClass} data-testid="chip-reporter">
          {task.reporter ? (
            <>
              <Avatar name={task.reporter.display_name} size="xs" src={avatarUrl(task.reporter.id)} />
              <span className="truncate">{task.reporter.display_name}</span>
            </>
          ) : (
            <span className="text-muted-foreground">No reporter</span>
          )}
        </span>
      ) : (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={chipClass} data-testid="chip-reporter">
            {task.reporter ? (
              <>
                <Avatar name={task.reporter.display_name} size="xs" src={avatarUrl(task.reporter.id)} />
                <span className="truncate">{task.reporter.display_name}</span>
              </>
            ) : (
              <span className="text-muted-foreground">No reporter</span>
            )}
            <ChevronDown className="h-3 w-3 text-muted-foreground" aria-hidden />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          {(members.data?.items ?? []).map((m) => (
            <DropdownMenuItem
              key={m.user.id}
              onSelect={() => onChange({ reporter_id: m.user.id }, "Reporter updated")}
              className="justify-between"
            >
              <span className="flex min-w-0 items-center gap-2">
                <Avatar name={m.user.display_name} size="xs" src={avatarUrl(m.user.id)} />
                <span className="truncate">{m.user.display_name}</span>
              </span>
              {task.reporter?.id === m.user.id ? <Check className="h-3.5 w-3.5" /> : null}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      )}

      {/* Assignee */}
      {readOnly ? (
        <span className={staticChipClass} data-testid="chip-assignee">
          {task.assignee ? (
            <>
              <Avatar name={task.assignee.display_name} size="xs" src={avatarUrl(task.assignee.id)} />
              <span className="truncate">{task.assignee.display_name}</span>
            </>
          ) : (
            <span className="text-muted-foreground">Unassigned</span>
          )}
        </span>
      ) : (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={chipClass} data-testid="chip-assignee">
            {task.assignee ? (
              <>
                <Avatar name={task.assignee.display_name} size="xs" src={avatarUrl(task.assignee.id)} />
                <span className="truncate">{task.assignee.display_name}</span>
              </>
            ) : (
              <span className="text-muted-foreground">
                <Plus className="mr-0.5 inline h-3 w-3" aria-hidden /> Assign
              </span>
            )}
            <ChevronDown className="h-3 w-3 text-muted-foreground" aria-hidden />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          <DropdownMenuItem
            onSelect={() => onChange({ assignee_id: "" }, "Assignee cleared")}
            data-testid="assignee-none"
          >
            <span className="text-muted-foreground">Unassigned</span>
          </DropdownMenuItem>
          {(members.data?.items ?? []).map((m) => (
            <DropdownMenuItem
              key={m.user.id}
              onSelect={() => onChange({ assignee_id: m.user.id }, "Assignee updated")}
              className="justify-between"
            >
              <span className="flex min-w-0 items-center gap-2">
                <Avatar name={m.user.display_name} size="xs" src={avatarUrl(m.user.id)} />
                <span className="truncate">{m.user.display_name}</span>
              </span>
              {task.assignee?.id === m.user.id ? <Check className="h-3.5 w-3.5" /> : null}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      )}

      {/* Priority */}
      {readOnly ? (
        <span className={staticChipClass} data-testid="chip-priority">
          <PriorityBars priority={task.priority} />
          {PRIORITY_LABELS[task.priority as Priority] ?? task.priority}
        </span>
      ) : (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={chipClass} data-testid="chip-priority">
            <PriorityBars priority={task.priority} />
            {PRIORITY_LABELS[task.priority as Priority] ?? task.priority}
            <ChevronDown className="h-3 w-3 text-muted-foreground" aria-hidden />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {PRIORITIES.map((p) => (
            <DropdownMenuItem
              key={p}
              onSelect={() => onChange({ priority: p }, "Priority updated")}
              className="justify-between"
              data-testid={`priority-${p}`}
            >
              {PRIORITY_LABELS[p]}
              {task.priority === p ? <Check className="h-3.5 w-3.5" /> : null}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      )}

      {/* Due date: chip swaps to an inline Doran picker. */}
      {readOnly ? (
        task.due_date ? (
          <span className={staticChipClass}>
            <CalendarDays className="h-3 w-3 text-muted-foreground" aria-hidden />
            {formatJalaliDate(task.due_date)}
          </span>
        ) : null
      ) : dueEditing ? (
        <span className="inline-flex items-center gap-1">
          <Suspense fallback={<Skeleton className="h-8 w-36" />}>
            <DateField
              id="chip-due-edit"
              value={task.due_date}
              onChange={(value) => {
                onChange({ due_date: value || null }, value ? "Due date updated" : "Due date cleared");
                setDueEditing(false);
              }}
              ariaLabel="Edit due date"
            />
          </Suspense>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Cancel due date edit"
            onClick={() => setDueEditing(false)}
          >
            <X />
          </Button>
        </span>
      ) : (
        <button
          type="button"
          className={chipClass}
          onClick={() => setDueEditing(true)}
          title={task.due_date ? `Due ${formatJalaliDate(task.due_date)}` : "Set a due date"}
          data-testid="chip-due"
        >
          <CalendarDays className="h-3 w-3 text-muted-foreground" aria-hidden />
          {task.due_date ? formatJalaliDate(task.due_date) : <span className="text-muted-foreground">Due date</span>}
        </button>
      )}

      {/* Deadline (WS3): hard cutoff, red-tinted; editable like the due date. */}
      {task.deadline || !readOnly ? (
        readOnly ? (
          task.deadline ? (
            <span className={deadlineChipClass} data-testid="chip-deadline">
              <Flag className="h-3 w-3" aria-hidden />
              {formatJalaliDate(task.deadline)}
            </span>
          ) : null
        ) : deadlineEditing ? (
          <span className="inline-flex items-center gap-1">
            <Suspense fallback={<Skeleton className="h-8 w-36" />}>
              <DateField
                id="chip-deadline-edit"
                value={task.deadline}
                onChange={(value) => {
                  onChange({ deadline: value || null }, value ? "Deadline updated" : "Deadline cleared");
                  setDeadlineEditing(false);
                }}
                ariaLabel="Edit deadline"
              />
            </Suspense>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Cancel deadline edit"
              onClick={() => setDeadlineEditing(false)}
            >
              <X />
            </Button>
          </span>
        ) : (
          <button
            type="button"
            className={deadlineChipClass}
            onClick={() => setDeadlineEditing(true)}
            title={task.deadline ? `Deadline ${formatJalaliDate(task.deadline)}` : "Set a deadline"}
            data-testid="chip-deadline"
          >
            <Flag className="h-3 w-3" aria-hidden />
            {task.deadline ? formatJalaliDate(task.deadline) : <span>Deadline</span>}
          </button>
        )
      ) : null}

      {/* Labels: toggle membership from the team label set. */}
      {readOnly ? (
        task.labels.length > 0 ? (
          <span className="inline-flex items-center gap-1">
            {task.labels.map((l) => (
              <LabelChip key={l.id} name={l.name} color={l.color} icon={l.icon} />
            ))}
          </span>
        ) : null
      ) : (
      <DropdownMenu open={labelsOpen} onOpenChange={setLabelsOpen}>
        <DropdownMenuTrigger asChild>
          <button type="button" className={chipClass} data-testid="chip-labels">
            {task.labels.length > 0 ? (
              <>
                {task.labels[0]?.icon ? (
                  <LabelIcon icon={task.labels[0].icon} />
                ) : (
                  <span className="h-2 w-2 rounded-full" style={{ backgroundColor: task.labels[0]?.color }} aria-hidden />
                )}
                <span className="truncate">
                  {task.labels[0]?.name}
                  {task.labels.length > 1 ? ` +${task.labels.length - 1}` : ""}
                </span>
              </>
            ) : (
              <span className="text-muted-foreground">
                <Plus className="mr-0.5 inline h-3 w-3" aria-hidden /> Label
              </span>
            )}
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-52">
          {(labels.data?.items ?? []).length === 0 ? (
            <DropdownMenuItem disabled className="text-2xs">
              No labels in this team yet
            </DropdownMenuItem>
          ) : null}
          {(labels.data?.items ?? []).map((l) => (
            <DropdownMenuItem key={l.id} onSelect={() => toggleLabel(l.id)} className="justify-between">
              <span className="flex items-center gap-2">
                <span className="h-2 w-2 rounded-full" style={{ backgroundColor: l.color }} aria-hidden />
                {l.name}
              </span>
              {selectedLabelIds.has(l.id) ? <Check className="h-3.5 w-3.5" /> : null}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      )}
    </div>
  );
}

function PriorityBars({ priority }: { priority: string }) {
  const level: Record<string, number> = { none: 0, low: 1, medium: 2, high: 3, urgent: 3, critical: 3 };
  const color: Record<string, string> = {
    none: "text-muted-foreground",
    low: "text-priority-low",
    medium: "text-priority-medium",
    high: "text-priority-high",
    urgent: "text-priority-urgent",
    critical: "text-priority-urgent",
  };
  const filled = level[priority] ?? 1;
  const tone = color[priority] ?? "text-muted-foreground";
  return (
    <span
      className={cn("flex items-end gap-px", priority === "critical" && "animate-pulse")}
      aria-hidden
    >
      {[3, 5, 7].map((h, i) => (
        <span
          key={h}
          style={{ height: `${h}px` }}
          className={cn("w-0.5 rounded-sm", i < filled ? tone : "bg-border")}
        />
      ))}
    </span>
  );
}

function PriorityBadgeInline({ priority, className }: { priority: string; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1 text-2xs text-muted-foreground", className)}>
      <PriorityBars priority={priority} />
      {PRIORITY_LABELS[priority as Priority] ?? priority}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Description: click to edit inline; Cmd/Ctrl+Enter saves, Escape cancels.
// ---------------------------------------------------------------------------

function DescriptionBlock({
  value,
  readOnly,
  onCommit,
}: {
  value: string;
  readOnly: boolean;
  onCommit: (description: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);

  const start = () => {
    setDraft(value);
    setEditing(true);
  };
  const commit = () => {
    setEditing(false);
    if (draft !== value) onCommit(draft);
  };

  if (editing) {
    // Phase 11: the description edits through the markdown editor (toolbar,
    // slash commands). Cmd/Ctrl+Enter saves; Escape cancels.
    return (
      <div className="mt-3" data-testid="description-editor">
        <MarkdownEditor
          value={draft}
          onChange={setDraft}
          onSubmit={commit}
          onCancel={() => setEditing(false)}
          rows={5}
          ariaLabel="Edit description"
          autoFocus
          testId="description-markdown"
        />
      </div>
    );
  }

  // WS10: closed tasks render the description as plain text.
  if (readOnly) {
    return (
      <div className="mt-3" data-testid="task-description">
        {value ? (
          <div className="px-2 py-1.5 text-sm leading-relaxed text-foreground/90">
            {renderMarkdown(value, "desc")}
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div
      className="mt-3 rounded-md transition-colors duration-100 hover:bg-accent/40"
      data-testid="task-description"
    >
      {value ? (
        <button
          type="button"
          onClick={start}
          className="w-full px-2 py-1.5 text-left text-sm leading-relaxed text-foreground/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="Edit description"
        >
          {renderMarkdown(value, "desc")}
        </button>
      ) : (
        <button
          type="button"
          onClick={start}
          className="w-full px-2 py-1.5 text-left text-sm text-muted-foreground/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          data-testid="description-empty"
        >
          Add a description…
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Activity: compact left-rule timeline, grouped by day, collapsed >10 events.
// ---------------------------------------------------------------------------

const ACTIVITY_COLLAPSE_AT = 10;

function ActivitySection({
  activity,
  isLoading,
  hoverTitle,
}: {
  activity: {
    id: string;
    created_at: number;
    event: string;
    field?: string | null;
    old_value?: string | null;
    new_value?: string | null;
    actor?: { id: string; display_name: string } | null;
  }[];
  isLoading: boolean;
  hoverTitle: (id: string | undefined, name: string) => string | undefined;
}) {
  const [showAll, setShowAll] = useState(false);

  const groups = useMemo(() => {
    const byDay = new Map<string, typeof activity>();
    for (const event of activity) {
      const day = new Intl.DateTimeFormat("en-GB", { dateStyle: "medium" }).format(
        new Date(event.created_at),
      );
      const list = byDay.get(day) ?? [];
      list.push(event);
      byDay.set(day, list);
    }
    return [...byDay.entries()];
  }, [activity]);

  const collapsed = !showAll && activity.length > ACTIVITY_COLLAPSE_AT;

  return (
    <section aria-label="Activity history">
      <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
        <History className="h-3.5 w-3.5" /> Activity
      </h3>
      {isLoading ? (
        <Skeleton className="h-20 w-full" />
      ) : activity.length === 0 ? (
        <p className="text-xs text-muted-foreground">No activity yet.</p>
      ) : (
        <>
          {(collapsed
            ? groups.map(([day, events]) => [day, events.slice(0, ACTIVITY_COLLAPSE_AT)] as const)
            : groups
          ).map(([day, events]) => (
            <div key={day} className="mb-3">
              <p className="mb-1.5 text-2xs font-medium uppercase tracking-wide text-muted-foreground">
                {day}
              </p>
              <ol className="flex flex-col gap-2.5 border-l pl-3">
                {events.map((event) => (
                  <li key={event.id} className="relative flex items-start gap-2.5 text-xs">
                    <span className="absolute -left-[17px] top-2 h-1.5 w-1.5 rounded-full bg-border" aria-hidden />
                    {event.actor ? (
                      <Avatar
                        name={event.actor.display_name}
                        size="xs"
                        src={avatarUrl(event.actor.id)}
                        title={hoverTitle(event.actor.id, event.actor.display_name)}
                      />
                    ) : (
                      <span className="flex h-5 w-5 items-center justify-center rounded-full bg-muted text-[9px]">?</span>
                    )}
                    <div>
                      <span className="font-medium">{event.actor?.display_name ?? "Someone"}</span>{" "}
                      <span className="text-muted-foreground">
                        {EVENT_LABELS[event.event] ?? event.event}
                      </span>
                      {event.field ? (
                        <div className="text-2xs text-muted-foreground">
                          {eventDetail(event.field, event.old_value, event.new_value)}
                        </div>
                      ) : null}
                      <div className="text-2xs text-muted-foreground"><Timestamp value={event.created_at} /></div>
                    </div>
                  </li>
                ))}
              </ol>
            </div>
          ))}
          {collapsed ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setShowAll(true)}
              data-testid="activity-show-all"
            >
              Show all {activity.length} events
            </Button>
          ) : null}
        </>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Checklist (Phase 12 WS4): lightweight checkbox items with a combined
// progress bar (closed subtasks + checked items). Read-only on closed tasks.
// ---------------------------------------------------------------------------

function ChecklistSection({
  readOnly,
  checklist,
  progress,
  onCreate,
  onToggle,
  onDelete,
}: {
  readOnly: boolean;
  checklist: { data?: { items: { id: string; title: string; is_checked: boolean }[] }; isLoading: boolean };
  progress?: {
    subtasks_total: number;
    subtasks_closed: number;
    checklist_total: number;
    checklist_checked: number;
    progress_pct?: number | null;
  } | null;
  onCreate: (title: string) => void;
  onToggle: (itemId: string, checked: boolean) => void;
  onDelete: (itemId: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const items = checklist.data?.items ?? [];

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const title = draft.trim();
    if (!title) return;
    onCreate(title);
    setDraft("");
  };

  const total = (progress?.subtasks_total ?? 0) + (progress?.checklist_total ?? 0);
  const done = (progress?.subtasks_closed ?? 0) + (progress?.checklist_checked ?? 0);
  const pct = progress?.progress_pct ?? null;

  return (
    <section className="mt-5" aria-label="Checklist" data-testid="task-checklist">
      <div className="mb-2 flex items-center justify-between gap-2">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold">
          Check
          <span className="text-muted-foreground">
            ({done}/{total})
          </span>
        </h3>
        {pct !== null && pct !== undefined ? (
          <span className="text-2xs tabular-nums text-muted-foreground" data-testid="checklist-progress">
            {pct}%
          </span>
        ) : null}
      </div>
      {total > 0 ? (
        <div
          className="mb-2 h-1.5 w-full overflow-hidden rounded-full bg-muted"
          role="progressbar"
          aria-valuenow={pct ?? 0}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label="Task progress"
        >
          <div
            className="h-full rounded-full bg-success transition-[width] duration-200"
            style={{ width: `${pct ?? 0}%` }}
          />
        </div>
      ) : null}
      <ul className="flex flex-col gap-0.5">
        {items.map((item) => (
          <li key={item.id} className="group flex items-center gap-2 rounded-md px-1 py-1 text-sm hover:bg-accent/40">
            <button
              type="button"
              role="checkbox"
              aria-checked={item.is_checked}
              aria-label={`Toggle ${item.title}`}
              disabled={readOnly}
              onClick={() => onToggle(item.id, !item.is_checked)}
              className={cn(
                "flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors",
                item.is_checked
                  ? "border-success bg-success text-background"
                  : "border-muted-foreground/40 hover:border-foreground/60",
                readOnly && "opacity-70",
              )}
              data-testid={`checklist-toggle-${item.id}`}
            >
              {item.is_checked ? <Check className="h-3 w-3" /> : null}
            </button>
            <span className={cn("min-w-0 flex-1 truncate", item.is_checked && "text-muted-foreground line-through")}>
              {item.title}
            </span>
            {!readOnly ? (
              <button
                type="button"
                aria-label={`Delete ${item.title}`}
                onClick={() => onDelete(item.id)}
                className="opacity-0 transition-opacity focus-visible:opacity-100 group-hover:opacity-100"
                data-testid={`checklist-delete-${item.id}`}
              >
                <Trash2 className="h-3.5 w-3.5 text-muted-foreground hover:text-destructive" />
              </button>
            ) : null}
          </li>
        ))}
      </ul>
      {!readOnly ? (
        <form onSubmit={submit} className="mt-1.5 flex items-center gap-1.5">
          <input
            type="text"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="Add checklist item…"
            maxLength={200}
            aria-label="New checklist item"
            className="h-7 w-full rounded-md border bg-transparent px-2 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            data-testid="checklist-input"
          />
          <Button type="submit" variant="ghost" size="icon-sm" aria-label="Add checklist item" disabled={!draft.trim()}>
            <Plus />
          </Button>
        </form>
      ) : null}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Attachments (Phase 12 WS11): file list with upload; read-only on closed
// tasks. Downloads open the cookie-authenticated endpoint directly.
// ---------------------------------------------------------------------------

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function AttachmentsSection({
  readOnly,
  attachments,
  onUpload,
  onDelete,
}: {
  readOnly: boolean;
  attachments: { data?: { items: { id: string; filename: string; mime_type: string; size_bytes: number; uploaded_by?: { display_name: string } | null }[] } };
  onUpload: (file: File) => void;
  onDelete: (attachmentId: string) => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const items = attachments.data?.items ?? [];

  return (
    <section className="mt-5" aria-label="Attachments" data-testid="task-attachments">
      <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
        <Paperclip className="h-3.5 w-3.5" /> Attachments ({items.length})
        {!readOnly ? (
          <>
            <input
              ref={inputRef}
              type="file"
              className="sr-only"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) onUpload(file);
                e.target.value = "";
              }}
              aria-label="Upload attachment"
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="ml-auto"
              aria-label="Upload attachment"
              onClick={() => inputRef.current?.click()}
              data-testid="attachment-upload"
            >
              <Plus />
            </Button>
          </>
        ) : null}
      </h3>
      {items.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          {readOnly ? "No files were attached." : "Drop in files via the + button above."}
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {items.map((a) => (
            <li key={a.id} className="group flex items-center gap-2 rounded-md border px-2 py-1.5 text-xs">
              <Paperclip className="h-3 w-3 shrink-0 text-muted-foreground" aria-hidden />
              <a
                href={api.attachmentUrl(a.id)}
                className="min-w-0 flex-1 truncate font-medium hover:underline"
                download
                data-testid={`attachment-${a.id}`}
              >
                {a.filename}
              </a>
              <span className="shrink-0 text-2xs text-muted-foreground">{formatBytes(a.size_bytes)}</span>
              {a.uploaded_by ? (
                <span className="hidden shrink-0 text-2xs text-muted-foreground sm:inline">
                  {a.uploaded_by.display_name}
                </span>
              ) : null}
              {!readOnly ? (
                <button
                  type="button"
                  aria-label={`Delete ${a.filename}`}
                  onClick={() => onDelete(a.id)}
                  className="opacity-0 transition-opacity focus-visible:opacity-100 group-hover:opacity-100"
                  data-testid={`attachment-delete-${a.id}`}
                >
                  <Trash2 className="h-3.5 w-3.5 text-muted-foreground hover:text-destructive" />
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
