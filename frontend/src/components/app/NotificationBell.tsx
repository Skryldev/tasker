import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { AtSign, Bell, CheckCheck, ClipboardList } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/badge";
import { EmptyState } from "@/components/app/EmptyState";
import { useNotifications, useNotificationMutations, useUnreadCount } from "@/api/notifications";
import type { Notification } from "@/api/client";
import { cn, timeAgo } from "@/lib/utils";

const EVENT_ICONS: Record<string, typeof Bell> = {
  task_assigned: ClipboardList,
  comment_mention: AtSign,
  task_submitted: ClipboardList,
  task_accepted: CheckCheck,
  task_rejected: ClipboardList,
  task_status_changed: ClipboardList,
  task_reporter_changed: ClipboardList,
};

export function NotificationBell() {
  const [open, setOpen] = useState(false);
  const [filterUnread, setFilterUnread] = useState(false);
  const navigate = useNavigate();
  const { data: unread } = useUnreadCount();
  const { data, isLoading, isError, refetch } = useNotifications(filterUnread);
  const { markRead, markAllRead } = useNotificationMutations();

  const count = unread?.count ?? 0;

  const openNotification = (notification: Notification) => {
    if (!notification.read_at) markRead.mutate(notification.id);
    setOpen(false);
    if (notification.task_id) {
      navigate(`/tasks/${notification.task_id}`);
    }
  };

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={`Notifications${count > 0 ? ` (${count} unread)` : ""}`}
          data-testid="notification-bell"
          className="relative"
        >
          <Bell />
          {count > 0 ? (
            <span
              data-testid="unread-badge"
              className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-destructive px-1 text-[9px] font-semibold text-destructive-foreground"
            >
              {count > 99 ? "99+" : count}
            </span>
          ) : null}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-96 max-w-[90vw] p-0">
        <div className="flex items-center justify-between px-3 py-2">
          <DropdownMenuLabel className="p-0">Notifications</DropdownMenuLabel>
          <div className="flex items-center gap-1">
            <Button
              variant={filterUnread ? "secondary" : "ghost"}
              size="sm"
              onClick={() => setFilterUnread((v) => !v)}
              aria-pressed={filterUnread}
            >
              Unread
            </Button>
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
              icon={<Bell className="h-6 w-6" />}
              title={filterUnread ? "No unread notifications" : "You're all caught up"}
              description="Assignments, mentions, and review requests will appear here."
            />
          ) : (
            data!.items.map((notification) => {
              const Icon = EVENT_ICONS[notification.event] ?? Bell;
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
                  <Icon className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
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
                      {timeAgo(notification.created_at)}
                    </span>
                  </span>
                </DropdownMenuItem>
              );
            })
          )}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
