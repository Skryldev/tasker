import { useState } from "react";
import type { Comment, Task } from "@/api/client";
import { Avatar } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { CommentBody, CommentComposer } from "./Mentions";
import { MarkdownEditor } from "./MarkdownEditor";
import { Pencil, Reply, Trash2 } from "lucide-react";
import { Timestamp } from "./Timestamp";
import { cn } from "@/lib/utils";

/**
 * Threaded replies (Phase 11): top-level comment + one nested reply level.
 * Depth is enforced server-side (422 on reply-to-reply); the UI never offers
 * a reply action on a reply.
 */

export interface CommentThreadData {
  parent: Comment;
  replies: Comment[];
}

export function buildThreads(items: Comment[]): CommentThreadData[] {
  const parents = items.filter((c) => !c.parent_comment_id);
  const repliesByParent = new Map<string, Comment[]>();
  for (const c of items) {
    if (c.parent_comment_id) {
      const list = repliesByParent.get(c.parent_comment_id) ?? [];
      list.push(c);
      repliesByParent.set(c.parent_comment_id, list);
    }
  }
  return parents.map((parent) => ({
    parent,
    replies: repliesByParent.get(parent.id) ?? [],
  }));
}

const COLLAPSE_REPLIES_AT = 3;

export function CommentThread({
  thread,
  task,
  hoverTitle,
  onReply,
  replyPending,
  onEdit,
  editingComment,
  setEditingComment,
  editDraft,
  setEditDraft,
  onDelete,
}: {
  thread: CommentThreadData;
  task: Task;
  hoverTitle: (id: string | undefined, name: string) => string | undefined;
  onReply: (body: string) => Promise<unknown>;
  replyPending: boolean;
  onEdit: (commentId: string, body: string) => Promise<unknown>;
  editingComment: string | null;
  setEditingComment: (id: string | null) => void;
  editDraft: string;
  setEditDraft: (v: string) => void;
  onDelete: (comment: Comment) => void;
}) {
  const { parent, replies } = thread;
  const [replyOpen, setReplyOpen] = useState(false);
  const [replyDraft, setReplyDraft] = useState("");
  const [showAll, setShowAll] = useState(false);
  const collapsed = !showAll && replies.length > COLLAPSE_REPLIES_AT;
  const visibleReplies = collapsed ? replies.slice(0, COLLAPSE_REPLIES_AT) : replies;

  return (
    <article className="border-b py-3 first:pt-0 last:border-0 group" data-testid={`comment-${parent.id}`}>
      <CommentRow
        comment={parent}
        parent
        hoverTitle={hoverTitle}
        onEdit={onEdit}
        editingComment={editingComment}
        setEditingComment={setEditingComment}
        editDraft={editDraft}
        setEditDraft={setEditDraft}
        onDelete={onDelete}
        onReplyClick={() => {
          setReplyOpen((v) => !v);
        }}
      />

      {/* Replies: nested list, 32px indent with a left rule. */}
      {replies.length > 0 ? (
        <ul className="ml-8 border-l pl-3" aria-label={`Replies to ${parent.author.display_name}`}>
          {visibleReplies.map((reply) => (
            <li key={reply.id} className="py-2 first:pt-1" data-testid={`reply-${reply.id}`}>
              <CommentRow
                comment={reply}
                parent={false}
                hoverTitle={hoverTitle}
                onEdit={onEdit}
                editingComment={editingComment}
                setEditingComment={setEditingComment}
                editDraft={editDraft}
                setEditDraft={setEditDraft}
                onDelete={onDelete}
              />
            </li>
          ))}
        </ul>
      ) : null}
      {collapsed ? (
        <button
          type="button"
          className="ml-8 mt-1 text-2xs text-primary hover:underline"
          onClick={() => setShowAll(true)}
          data-testid="show-replies"
        >
          Show {replies.length - COLLAPSE_REPLIES_AT} more repl
          {replies.length - COLLAPSE_REPLIES_AT === 1 ? "y" : "ies"}
        </button>
      ) : null}

      {/* Inline reply composer: 150ms slide-down + fade (transform/opacity). */}
      <div
        className={cn(
          "ml-8 origin-top transition-all duration-150 ease-out",
          replyOpen ? "visible translate-y-0 opacity-100" : "invisible -translate-y-1 opacity-0",
        )}
      >
        {replyOpen ? (
          <form
            className="mt-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (!replyDraft.trim()) return;
              void onReply(replyDraft.trim()).then(() => {
                setReplyDraft("");
                setReplyOpen(false);
              });
            }}
            data-testid={`reply-form-${parent.id}`}
          >
            <CommentComposer
              teamId={task.team_id}
              value={replyDraft}
              onChange={setReplyDraft}
              onSubmit={() => void onReply(replyDraft.trim())}
              pending={replyPending}
              testIdInput="reply-composer-input"
              testIdSubmit="reply-composer-submit"
            />
            <div className="mt-1 flex gap-2">
              <Button
                type="submit"
                size="sm"
                disabled={replyPending || !replyDraft.trim()}
                data-testid={`reply-submit-${parent.id}`}
              >
                Reply
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={() => {
                  setReplyOpen(false);
                  setReplyDraft("");
                }}
              >
                Cancel
              </Button>
            </div>
          </form>
        ) : null}
      </div>
    </article>
  );
}

function CommentRow({
  comment,
  parent,
  hoverTitle,
  onEdit,
  editingComment,
  setEditingComment,
  editDraft,
  setEditDraft,
  onDelete,
  onReplyClick,
}: {
  comment: Comment;
  parent: boolean;
  hoverTitle: (id: string | undefined, name: string) => string | undefined;
  onEdit: (commentId: string, body: string) => Promise<unknown>;
  editingComment: string | null;
  setEditingComment: (id: string | null) => void;
  editDraft: string;
  setEditDraft: (v: string) => void;
  onDelete: (comment: Comment) => void;
  onReplyClick?: () => void;
}) {
  return (
    <>
      <header className="flex items-center gap-2 text-xs">
        <Avatar
          name={comment.author.display_name}
          size={parent ? "sm" : "xs"}
          title={hoverTitle(comment.author.id, comment.author.display_name)}
        />
        <span className={parent ? "text-[13px] font-semibold" : "text-[13px] font-normal"}>
          {comment.author.display_name}
        </span>
        <span className="text-muted-foreground">
          <Timestamp value={comment.created_at} />
        </span>
        {comment.created_at !== comment.updated_at ? (
          <span className="text-muted-foreground">(edited)</span>
        ) : null}
        <span className="ml-auto flex gap-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-within:opacity-100">
          {onReplyClick ? (
            <button
              type="button"
              className="rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground"
              onClick={onReplyClick}
              aria-label={`Reply to ${comment.author.display_name}`}
              data-testid={`reply-to-${comment.id}`}
            >
              <Reply className="h-3 w-3" />
            </button>
          ) : null}
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
            onClick={() => onDelete(comment)}
            aria-label="Delete comment"
          >
            <Trash2 className="h-3 w-3" />
          </button>
        </span>
      </header>
      {editingComment === comment.id ? (
        <div className="mt-2">
          <MarkdownEditor
            value={editDraft}
            onChange={setEditDraft}
            onSubmit={() => void onEdit(comment.id, editDraft)}
            onCancel={() => setEditingComment(null)}
            rows={2}
            ariaLabel="Edit comment"
          />
          <div className="mt-1 flex gap-2">
            <Button
              type="button"
              size="sm"
              onClick={() => void onEdit(comment.id, editDraft)}
              data-testid="comment-edit-save"
            >
              Save
            </Button>
            <Button type="button" size="sm" variant="ghost" onClick={() => setEditingComment(null)}>
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <CommentBody body={comment.body} teamId={comment.task_id} />
      )}
    </>
  );
}

