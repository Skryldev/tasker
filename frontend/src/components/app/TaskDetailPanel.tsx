import { useState } from "react";
import { CalendarDays, History, MessageSquare, Pencil, ListTree, Trash2 } from "lucide-react";
import { api, type Task } from "@/api/client";
import {
  useActivity,
  useComments,
  useCreateComment,
  useDeleteComment,
  useDeleteTask,
  useUpdateComment,
} from "@/api/hooks";
import { Avatar, Separator } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/badge";
import { StatusBadge, PriorityBadge } from "./TaskBadges";
import { TaskDialog } from "./TaskDialog";
import { useToast } from "@/components/ui/toast";
import { cn, formatDate, timeAgo } from "@/lib/utils";
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
}: {
  task: Task | undefined;
  onClose: () => void;
}) {
  const [editOpen, setEditOpen] = useState(false);
  const [commentDraft, setCommentDraft] = useState("");
  const [editingComment, setEditingComment] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const { push } = useToast();
  const qc = useQueryClient();

  const taskId = task?.id;
  const comments = useComments(taskId);
  const activity = useActivity(taskId);
  const createComment = useCreateComment(taskId ?? "");
  const updateComment = useUpdateComment(taskId ?? "");
  const deleteComment = useDeleteComment(taskId ?? "");
  const deleteTask = useDeleteTask();

  if (!task) {
    return (
      <div className="flex w-full flex-col items-center justify-center gap-3 p-10 text-sm text-muted-foreground">
        <History className="h-8 w-8 opacity-40" />
        Select a task to see its details.
      </div>
    );
  }

  const submitComment = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!commentDraft.trim()) return;
    try {
      await createComment.mutateAsync(commentDraft.trim());
      setCommentDraft("");
    } catch (error) {
      push("error", "Could not add comment", error instanceof Error ? error.message : undefined);
    }
  };

  const confirmDeleteTask = () => {
    if (!window.confirm(`Delete task "${task.title}"? Its subtasks are deleted too.`)) return;
    deleteTask.mutate(task.id, {
      onSuccess: () => {
        push("info", "Task deleted");
        onClose();
      },
      onError: (error) =>
        push("error", "Could not delete task", error instanceof Error ? error.message : undefined),
    });
  };

  return (
    <div className="flex h-full flex-col" data-testid="task-detail">
      <div className="flex items-center justify-between gap-2 border-b p-4">
        <p className="text-2xs uppercase tracking-wide text-muted-foreground">Task detail</p>
        <div className="flex items-center gap-1">
          <Button variant="ghost" size="icon-sm" onClick={() => setEditOpen(true)} aria-label="Edit task" data-testid="edit-task">
            <Pencil />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={confirmDeleteTask}
            aria-label="Delete task"
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 />
          </Button>
          <Button variant="ghost" size="icon-sm" onClick={onClose} aria-label="Close detail panel">
            ✕
          </Button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto scrollbar-thin p-4">
        <h2 className="text-base font-semibold leading-snug">{task.title}</h2>
        {task.description ? (
          <p className="mt-2 whitespace-pre-wrap text-sm text-muted-foreground">{task.description}</p>
        ) : null}

        <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
          <StatusBadge status={task.status} />
          <PriorityBadge priority={task.priority} />
          <span className="inline-flex items-center gap-1">
            <CalendarDays className="h-3.5 w-3.5" /> Due {formatDate(task.due_date)}
          </span>
          {task.assignee ? (
            <span className="inline-flex items-center gap-1.5">
              <Avatar name={task.assignee.display_name} size="xs" />
              {task.assignee.display_name}
            </span>
          ) : (
            <span>Unassigned</span>
          )}
        </div>

        {task.labels.length > 0 ? (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {task.labels.map((l) => (
              <span
                key={l.id}
                className="inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-2xs"
              >
                <span className="h-2 w-2 rounded-full" style={{ backgroundColor: l.color }} />
                {l.name}
              </span>
            ))}
          </div>
        ) : null}

        {task.subtasks && task.subtasks.length > 0 ? (
          <section className="mt-5" aria-label="ListTree">
            <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
              <ListTree className="h-3.5 w-3.5" /> ListTree ({task.subtasks.length})
            </h3>
            <ul className="flex flex-col gap-1">
              {task.subtasks.map((sub) => (
                <li key={sub.id} className="flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-sm">
                  <StatusBadge status={sub.status} />
                  <span className={cn("truncate", sub.status === "done" && "text-muted-foreground line-through")}>
                    {sub.title}
                  </span>
                  <PriorityBadge priority={sub.priority} className="ml-auto" />
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        <Separator className="my-5" />

        <section aria-label="Comments" className="mt-1">
          <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
            <MessageSquare className="h-3.5 w-3.5" /> Comments ({comments.data?.total ?? 0})
          </h3>
          <form onSubmit={submitComment} className="mb-4 flex flex-col gap-2">
            <textarea
              value={commentDraft}
              onChange={(e) => setCommentDraft(e.target.value)}
              placeholder="Leave a comment…"
              rows={2}
              className="w-full rounded-md border border-input bg-background px-2.5 py-1.5 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              data-testid="comment-input"
            />
            <Button type="submit" size="sm" className="self-start" disabled={!commentDraft.trim() || createComment.isPending} data-testid="comment-submit">
              Comment
            </Button>
          </form>
          <div className="flex flex-col gap-3">
            {comments.isLoading ? (
              <>
                <Skeleton className="h-14 w-full" />
                <Skeleton className="h-14 w-full" />
              </>
            ) : (
              comments.data?.items.map((comment) => (
                <article key={comment.id} className="rounded-md border p-3">
                  <header className="flex items-center gap-2 text-xs">
                    <Avatar name={comment.author.display_name} size="xs" />
                    <span className="font-medium">{comment.author.display_name}</span>
                    <span className="text-muted-foreground">{timeAgo(comment.created_at)}</span>
                    {comment.created_at !== comment.updated_at ? (
                      <span className="text-muted-foreground">(edited)</span>
                    ) : null}
                    <span className="ml-auto flex gap-1">
                      <button
                        type="button"
                        className="rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                        onClick={() => {
                          setEditingComment(comment.id);
                          setEditDraft(comment.body);
                        }}
                        aria-label="Edit comment"
                      >
                        <Pencil className="h-3 w-3" />
                      </button>
                      <button
                        type="button"
                        className="rounded px-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                        onClick={() =>
                          deleteComment.mutate(comment.id, {
                            onError: (error) =>
                              push("error", "Could not delete comment", error instanceof Error ? error.message : undefined),
                          })
                        }
                        aria-label="Delete comment"
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </span>
                  </header>
                  {editingComment === comment.id ? (
                    <form
                      className="mt-2 flex flex-col gap-2"
                      onSubmit={async (e) => {
                        e.preventDefault();
                        await updateComment.mutateAsync({ commentId: comment.id, body: editDraft });
                        setEditingComment(null);
                      }}
                    >
                      <textarea
                        value={editDraft}
                        onChange={(e) => setEditDraft(e.target.value)}
                        rows={2}
                        className="w-full rounded-md border border-input bg-background px-2 py-1 text-sm"
                      />
                      <div className="flex gap-2">
                        <Button type="submit" size="sm">Save</Button>
                        <Button type="button" size="sm" variant="ghost" onClick={() => setEditingComment(null)}>
                          Cancel
                        </Button>
                      </div>
                    </form>
                  ) : (
                    <p className="mt-1.5 whitespace-pre-wrap text-sm">{comment.body}</p>
                  )}
                </article>
              ))
            )}
          </div>
        </section>

        <Separator className="my-5" />

        <section aria-label="Activity history">
          <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold">
            <History className="h-3.5 w-3.5" /> Activity
          </h3>
          {activity.isLoading ? (
            <Skeleton className="h-20 w-full" />
          ) : (
            <ol className="flex flex-col gap-2.5">
              {activity.data?.items.map((event) => (
                <li key={event.id} className="flex items-start gap-2.5 text-xs">
                  {event.actor ? (
                    <Avatar name={event.actor.display_name} size="xs" />
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
                    <div className="text-2xs text-muted-foreground">{timeAgo(event.created_at)}</div>
                  </div>
                </li>
              ))}
              {activity.data?.items.length === 0 ? (
                <li className="text-xs text-muted-foreground">No activity yet.</li>
              ) : null}
            </ol>
          )}
        </section>
      </div>

      {editOpen ? (
        <TaskDialogResolver
          open={editOpen}
          onOpenChange={setEditOpen}
          taskId={task.id}
          onSaved={async () => {
            const fresh = await api.getTask(task.id);
            void qc.setQueryData(["task", task.id], fresh);
          }}
        />
      ) : null}
    </div>
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
