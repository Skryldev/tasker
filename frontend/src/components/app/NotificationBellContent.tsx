import { useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { AtSign, Bell, CheckCheck, ClipboardList, Inbox, Stamp } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/badge";
import { EmptyState } from "@/components/app/EmptyState";
import { useNotifications, useNotificationMutations } from "@/api/notifications";
import type { Notification } from "@/api/client";
import { cn } from "@/lib/utils";
import { Timestamp } from "@/components/app/Timestamp";

/** WS12: each event type gets its own tinted icon chip. */
const EVENT_ICONS: Record<string, { Icon: typeof Bell; tone: string }> = {
  task_assigned: { Icon: ClipboardList, tone: "bg-primary/10 text-primary" },
  comment_mention: { Icon: AtSign, tone: "bg-warning/10 text-warning" },
  task_submitted: { Icon: Stamp, tone: "bg-status-in-review/15 text-status-in-review" },
  task_accepted: { Icon: CheckCheck, tone: "bg-success/10 text-success" },
  task_rejected: { Icon: ClipboardList, tone: "bg-destructive/10 text-destructive" },
  task_status_changed: { Icon: ClipboardList, tone: "bg-muted text-muted-foreground" },
  task_reporter_changed: { Icon: ClipboardList, tone: "bg-muted text-muted-foreground" },
};

/** Day headings like the activity feed: Today / Yesterday / medium date. */
function dayLabel(ms: number): string {
  const fmt = new Intl.DateTimeFormat("en-GB", { dateStyle: "medium" });
  const today = fmt.format(new Date());
  const yesterday = fmt.format(new Date(Date.now() - 86_400_000));
  const day = fmt.format(new Date(ms));
  if (day === today) return "Today";
  if (day === yesterday) return "Yesterday";
  return day;
}

export function NotificationBellContent({
  filterUnread,
  onFilterChange,
  onClose,
  unreadCount,
}: {
  filterUnread: boolean;
  onFilterChange: (unread: boolean) => void;
  onClose: () => void;
  unreadCount: number;
}) {
  const navigate = useNavigate();
  const { data, isLoading, isError, refetch } = useNotifications(filterUnread);
  const { markRead, markAllRead } = useNotificationMutations();
  const count = unreadCount;

  // WS12: group the visible items under day headings.
  const groups = useMemo(() => {
    const items = data?.items ?? [];
    const byDay = new Map<string, Notification[]>();
    for (const n of items) {
      const day = dayLabel(n.created_at);
      const list = byDay.get(day) ?? [];
      list.push(n);
      byDay.set(day, list);
    }
    return [...byDay.entries()];
  }, [data]);

  const openNotification = (notification: Notification) => {
    if (!notification.read_at) markRead.mutate(notification.id);
    onClose();
    if (notification.task_id) {
      navigate(`/tasks/${notification.task_id}`);
    }
  };

  return (
    <DropdownMenuContent align="end" className="w-96 max-w-[90vw] p-0">
      <div className="flex items-center justify-between px-3 py-2">
        <DropdownMenuLabel className="p-0">Notifications</DropdownMenuLabel>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => markAllRead.mutate()}
          disabled={count === 0}
          data-testid="mark-all-read"
        >
          <CheckCheck /> Mark all read
        </Button>
      </div>
      {/* WS12: filter tabs — All / Unread. */}
      <div className="flex gap-1 px-3 pb-2" role="tablist" aria-label="Notification filter">
        {[
          { id: "all", label: "All" },
          { id: "unread", label: `Unread${count > 0 ? ` (${count})` : ""}` },
        ].map((tab) => {
          const active = (tab.id === "unread") === filterUnread;
          return (
            <Button
              key={tab.id}
              role="tab"
              aria-selected={active}
              variant={active ? "secondary" : "ghost"}
              size="sm"
              onClick={() => onFilterChange(tab.id === "unread")}
              data-testid={`notification-tab-${tab.id}`}
            >
              {tab.label}
            </Button>
          );
        })}
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
        ) : (data?.items.length ?? 0) === 0 ? (
          <EmptyState
            icon={<Inbox className="h-6 w-6" />}
            title={filterUnread ? "No unread notifications" : "You're all caught up"}
            description="Assignments, mentions, and review requests will appear here."
          />
        ) : (
          groups.map(([day, items]) => (
            <section key={day} aria-label={day}>
              <p className="sticky top-0 z-10 bg-popover/95 px-3 py-1 text-2xs font-medium uppercase tracking-wide text-muted-foreground backdrop-blur">
                {day}
              </p>
              {items.map((notification) => {
                const { Icon, tone } = EVENT_ICONS[notification.event] ?? {
                  Icon: Bell,
                  tone: "bg-muted text-muted-foreground",
                };
                return (
                  <DropdownMenuItem
                    key={notification.id}
                    onSelect={() => openNotification(notification)}
                    className={cn(
                      "flex cursor-pointer items-start gap-2.5 rounded-none border-b px-3 py-2.5 last:border-0",
                      !notification.read_at && "bg-primary/5",
                    )}
                    data-testid={`notification-${notification.id}`}
                  >
                    <span
                      className={cn(
                        "mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full",
                        tone,
                      )}
                      aria-hidden
                    >
                      <Icon className="h-3.5 w-3.5" />
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
                      </span>
                      <span className="text-2xs text-muted-foreground">
                        <Timestamp value={notification.created_at} />
                      </span>
                    </span>
                  </DropdownMenuItem>
                );
              })}
            </section>
          ))
        )}
      </div>
    </DropdownMenuContent>
  );
}
