import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  ArrowRight,
  AtSign,
  Bell,
  CalendarClock,
  CheckCheck,
  ClipboardList,
  Inbox,
  MessageSquare,
  Pin,
  PinOff,
  Search,
  Stamp,
  XCircle,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/badge";
import { EmptyState } from "@/components/app/EmptyState";
import {
  eventTab,
  useNotificationTabCounts,
  useNotifications,
  useNotificationMutations,
  usePinnedNotifications,
  type NotificationTab,
} from "@/api/notifications";
import type { Notification } from "@/api/client";
import { cn } from "@/lib/utils";
import { Timestamp } from "@/components/app/Timestamp";

/**
 * Notification Action Center (Phase 14 WS5, ADR-0022): tabs with counts,
 * search, a pinned section above time groups (Today / Yesterday / This
 * Week / Earlier), per-type tinted icons with a left color bar,
 * client-side batching of bursts (same actor + task within 5 minutes),
 * and quick actions that deep-link into the task page.
 */

const PAGE_SIZE = 50;

/** Per-type identity: icon + accent color (the left bar + icon chip). */
const EVENT_META: Record<string, { Icon: LucideIcon; color: string }> = {
  task_assigned: { Icon: ClipboardList, color: "#3b82f6" },
  comment_mention: { Icon: AtSign, color: "#f59e0b" },
  task_submitted: { Icon: Stamp, color: "#8b5cf6" },
  task_accepted: { Icon: CheckCheck, color: "#10b981" },
  task_rejected: { Icon: XCircle, color: "#ef4444" },
  task_status_changed: { Icon: ArrowRight, color: "#64748b" },
  task_reporter_changed: { Icon: ClipboardList, color: "#64748b" },
};

const FALLBACK_META = { Icon: Bell, color: "#64748b" } as const;

/** Action-center time groups. */
function timeGroup(ms: number): string {
  const day = new Date(ms);
  day.setHours(0, 0, 0, 0);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const diffDays = Math.round((today.getTime() - day.getTime()) / 86_400_000);
  if (diffDays <= 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) return "This Week";
  return "Earlier";
}

const GROUP_ORDER = ["Today", "Yesterday", "This Week", "Earlier"] as const;

/** A batch: one leader notification plus the quiet followers. */
interface Batch {
  leader: Notification;
  followers: Notification[];
}

const BATCH_WINDOW_MS = 5 * 60 * 1000;

/**
 * Client-side smart batching (ADR-0022): consecutive notifications from
 * the same actor on the same task within 5 minutes collapse into one card
 * with a "+N" chip. Expanding reveals the followers.
 */
function batchNotifications(items: Notification[]): { batches: Batch[]; singles: Notification[] } {
  const batches: Batch[] = [];
  const singles: Notification[] = [];
  let current: Batch | null = null;
  const ordered = [...items].sort((a, b) => b.created_at - a.created_at);
  for (const n of ordered) {
    const last = current?.leader;
    const sameBurst =
      last !== undefined &&
      last.actor?.id !== undefined &&
      last.actor?.id === n.actor?.id &&
      last.task_id === n.task_id &&
      last.created_at - n.created_at < BATCH_WINDOW_MS;
    if (sameBurst && current) {
      current.followers.push(n);
    } else {
      if (current) batches.push(current);
      current = { leader: n, followers: [] };
    }
  }
  if (current) {
    if (current.followers.length > 0) batches.push(current);
    else singles.push(current.leader);
  }
  return { batches, singles };
}

const TABS: { id: NotificationTab; label: string }[] = [
  { id: "all", label: "All" },
  { id: "unread", label: "Unread" },
  { id: "mentions", label: "Mentions" },
  { id: "assigned", label: "Assigned" },
  { id: "activity", label: "Task Activity" },
];

export function NotificationBellContent({
  tab,
  onTabChange,
  onClose,
  unreadCount,
}: {
  tab: NotificationTab;
  onTabChange: (tab: NotificationTab) => void;
  onClose: () => void;
  unreadCount: number;
}) {
  const navigate = useNavigate();
  const [page, setPage] = useState(1);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const { data, isLoading, isError, refetch } = useNotifications(tab, page);
  const pinned = usePinnedNotifications(true);
  const counts = useNotificationTabCounts(true);
  const { markRead, markAllRead, togglePin } = useNotificationMutations();

  const items = data?.items ?? [];
  const filtered = useMemo(() => {
    // Mentions/Assigned/Activity tabs filter the loaded set client-side
    // (ADR-0022); search narrows it further.
    let list =
      tab === "all" || tab === "unread" ? items : items.filter((n) => eventTab(n.event) === tab);
    const q = query.trim().toLowerCase();
    if (q) {
      list = list.filter(
        (n) =>
          n.summary.toLowerCase().includes(q) ||
          (n.actor?.display_name ?? "").toLowerCase().includes(q),
      );
    }
    return list;
  }, [items, query, tab]);

  const { batches, singles } = useMemo(() => batchNotifications(filtered), [filtered]);

  const groups = useMemo(() => {
    const byGroup = new Map<string, Notification[]>();
    for (const n of filtered) {
      const g = timeGroup(n.created_at);
      const list = byGroup.get(g) ?? [];
      list.push(n);
      byGroup.set(g, list);
    }
    return GROUP_ORDER.filter((g) => byGroup.has(g)).map(
      (g) => [g, byGroup.get(g)!] as const,
    );
  }, [filtered]);

  const totalLoaded = items.length;
  const totalAll = data?.total ?? 0;
  const hasMore = page * PAGE_SIZE < totalAll;

  const openNotification = (notification: Notification) => {
    if (!notification.read_at) markRead.mutate(notification.id);
    onClose();
    if (notification.task_id) {
      navigate(`/tasks/${notification.task_id}`);
    }
  };

  const focusTask = (notification: Notification, focus: "comments" | "dates") => {
    if (!notification.read_at) markRead.mutate(notification.id);
    onClose();
    navigate(`/tasks/${notification.task_id}?focus=${focus}`);
  };

  const renderRow = (notification: Notification, extraCount = 0, followers: Notification[] = []) => {
    const meta = EVENT_META[notification.event] ?? FALLBACK_META;
    const batchId = notification.id;
    const isOpen = expanded.has(batchId);
    return (
      <div key={notification.id} className="border-b last:border-0">
        <div
          className={cn(
            "relative flex cursor-pointer items-start gap-2.5 px-3 py-2.5 hover:bg-accent/40",
            !notification.read_at && "bg-primary/5",
          )}
          style={{ borderLeft: `3px solid ${meta.color}` }}
          role="button"
          tabIndex={0}
          onClick={() => openNotification(notification)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              openNotification(notification);
            }
          }}
          data-testid={`notification-${notification.id}`}
        >
          <span
            className={cn(
              "mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full",
            )}
            style={{ backgroundColor: `${meta.color}1f`, color: meta.color }}
            aria-hidden
          >
            <meta.Icon className="h-3.5 w-3.5" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-2">
              <span
                className={cn(
                  "truncate text-sm",
                  notification.read_at ? "text-muted-foreground" : "font-medium",
                )}
              >
                {notification.summary}
              </span>
              {!notification.read_at ? (
                <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary" aria-label="unread" />
              ) : null}
              {extraCount > 0 ? (
                <span
                  className="shrink-0 rounded-full bg-muted px-1.5 text-2xs font-medium text-muted-foreground"
                  aria-label={`${extraCount} more in this burst`}
                >
                  +{extraCount}
                </span>
              ) : null}
            </span>
            <span className="flex items-center gap-2 text-2xs text-muted-foreground">
              <Timestamp value={notification.created_at} />
              {notification.actor?.display_name ? (
                <span className="truncate">{notification.actor.display_name}</span>
              ) : null}
            </span>
          </span>
          <span
            className="flex shrink-0 items-center gap-0.5"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => e.stopPropagation()}
          >
            <QuickAction
              label={notification.event === "comment_mention" ? "Reply" : "Reschedule"}
              title={
                notification.event === "comment_mention"
                  ? "Reply in the discussion"
                  : "Reschedule the task"
              }
              onClick={() =>
                focusTask(notification, notification.event === "comment_mention" ? "comments" : "dates")
              }
              icon={
                notification.event === "comment_mention" ? (
                  <MessageSquare className="h-3.5 w-3.5" />
                ) : (
                  <CalendarClock className="h-3.5 w-3.5" />
                )
              }
              testId={`quick-${notification.id}`}
            />
            <QuickAction
              label="View Task"
              title="Open the task"
              onClick={() => openNotification(notification)}
              icon={<ArrowRight className="h-3.5 w-3.5" />}
              testId={`view-${notification.id}`}
            />
            <QuickAction
              label={notification.pinned ? "Unpin" : "Pin"}
              title={notification.pinned ? "Unpin notification" : "Pin notification"}
              onClick={() => togglePin.mutate({ id: notification.id, pinned: !notification.pinned })}
              icon={notification.pinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
              testId={`pin-${notification.id}`}
            />
          </span>
        </div>
        {extraCount > 0 && isOpen
          ? followers.map((f) => (
              <div
                key={f.id}
                className={cn(
                  "flex cursor-pointer items-start gap-2.5 border-t px-3 py-1.5 pl-12 hover:bg-accent/40",
                  !f.read_at && "bg-primary/5",
                )}
                onClick={() => openNotification(f)}
                data-testid={`notification-${f.id}`}
              >
                <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                  {f.summary}
                </span>
                <span className="shrink-0 text-2xs text-muted-foreground">
                  <Timestamp value={f.created_at} />
                </span>
              </div>
            ))
          : null}
        {extraCount > 0 && !isOpen ? (
          <button
            type="button"
            className="w-full border-t px-3 py-1 text-left text-2xs text-primary hover:bg-accent/40"
            onClick={() =>
              setExpanded((prev) => {
                const next = new Set(prev);
                next.add(batchId);
                return next;
              })
            }
            data-testid={`expand-batch-${batchId}`}
          >
            Show {extraCount} more
          </button>
        ) : null}
      </div>
    );
  };

  const pinnedItems = pinned.data?.items ?? [];
  const anyContent = batches.length > 0 || singles.length > 0;

  return (
    <DropdownMenuContent align="end" className="w-[26rem] max-w-[92vw] p-0">
      <div className="flex items-center justify-between px-3 py-2">
        <DropdownMenuLabel className="p-0">Notifications</DropdownMenuLabel>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => markAllRead.mutate()}
          disabled={unreadCount === 0}
          data-testid="mark-all-read"
        >
          <CheckCheck /> Mark all read
        </Button>
      </div>
      {/* Action center tabs (ADR-0022) with live counts. */}
      <div className="flex flex-wrap gap-1 px-3 pb-2" role="tablist" aria-label="Notification filter">
        {TABS.map((t) => {
          const active = t.id === tab;
          const tabCount =
            t.id === "all" || t.id === "unread"
              ? unreadCount
              : counts.data?.[t.id]?.unread ?? 0;
          return (
            <Button
              key={t.id}
              role="tab"
              aria-selected={active}
              variant={active ? "secondary" : "ghost"}
              size="sm"
              className="h-7 px-2 text-xs"
              onClick={() => {
                onTabChange(t.id);
                setPage(1);
              }}
              data-testid={`notification-tab-${t.id}`}
            >
              {t.label}
              {tabCount > 0 ? (
                <span className="ml-1 rounded-full bg-primary/15 px-1.5 text-2xs font-semibold text-primary">
                  {tabCount > 99 ? "99+" : tabCount}
                </span>
              ) : null}
            </Button>
          );
        })}
      </div>
      <div className="relative px-3 pb-2">
        <Search
          className="pointer-events-none absolute left-5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
          aria-hidden
        />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search notifications…"
          aria-label="Search notifications"
          className="h-8 w-full rounded-md border bg-background pl-8 pr-2 text-xs outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-primary"
          data-testid="notification-search"
        />
      </div>
      <DropdownMenuSeparator className="my-0" />
      <div className="max-h-96 overflow-y-auto scrollbar-thin" data-testid="notification-list">
        {isLoading ? (
          <div className="flex flex-col gap-2 p-3">
            <Skeleton className="h-12 w-full" />
            <Skeleton className="h-12 w-full" />
            <Skeleton className="h-12 w-full" />
          </div>
        ) : isError ? (
          <div className="p-4 text-center text-xs text-muted-foreground" role="alert">
            Could not load notifications.{" "}
            <button className="text-primary underline" onClick={() => void refetch()}>
              Retry
            </button>
          </div>
        ) : !anyContent ? (
          <EmptyState
            icon={<Inbox className="h-6 w-6" />}
            title={query ? "No matches" : "You're all caught up"}
            description="Assignments, mentions, and review requests will appear here."
          />
        ) : (
          <>
            {pinnedItems.length > 0 ? (
              <section aria-label="Pinned">
                <p className="sticky top-0 z-10 bg-popover/95 px-3 py-1 text-2xs font-medium uppercase tracking-wide text-muted-foreground backdrop-blur">
                  Pinned
                </p>
                {pinnedItems
                  .filter((p) => !filtered.some((n) => n.id === p.id))
                  .map((p) => renderRow(p))}
              </section>
            ) : null}
            {groups.map(([group, groupItems]) => {
              // Re-batch within the group so bursts never span time headers.
              const { batches: gBatches, singles: gSingles } = batchNotifications(groupItems);
              return (
                <section key={group} aria-label={group}>
                  <p className="sticky top-0 z-10 bg-popover/95 px-3 py-1 text-2xs font-medium uppercase tracking-wide text-muted-foreground backdrop-blur">
                    {group}
                  </p>
                  {gBatches.map((b) =>
                    expanded.has(b.leader.id) ? (
                      <div key={b.leader.id}>
                        {renderRow(b.leader, b.followers.length, b.followers)}
                      </div>
                    ) : (
                      renderRow(b.leader, b.followers.length)
                    ),
                  )}
                  {gSingles.map((s) => renderRow(s))}
                </section>
              );
            })}
            {hasMore && !query ? (
              <div className="p-2 text-center">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setPage((p) => p + 1)}
                  data-testid="notifications-load-more"
                >
                  Load more ({totalAll - totalLoaded} remaining)
                </Button>
              </div>
            ) : null}
          </>
        )}
      </div>
    </DropdownMenuContent>
  );
}

function QuickAction({
  label,
  title,
  onClick,
  icon,
  testId,
}: {
  label: string;
  title: string;
  onClick: () => void;
  icon: React.ReactNode;
  testId: string;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={title}
      onClick={onClick}
      className="flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
      data-testid={testId}
    >
      {icon}
    </button>
  );
}
