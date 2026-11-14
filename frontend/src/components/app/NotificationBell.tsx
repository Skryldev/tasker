import { lazy, Suspense, useState } from "react";
import { Bell } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useUnreadCount, type NotificationTab } from "@/api/notifications";

// Phase 12 WS12; the action center (Phase 14 WS5) stays lazy — the eager
// shell is just the bell + unread badge, keeping the main bundle small.
const NotificationBellContent = lazy(() =>
  import("./NotificationBellContent").then((m) => ({ default: m.NotificationBellContent })),
);

export function NotificationBell() {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<NotificationTab>("all");
  const [opened, setOpened] = useState(false);
  const { data: unread } = useUnreadCount();

  const count = unread?.count ?? 0;

  return (
    <DropdownMenu
      open={open}
      onOpenChange={(v) => {
        setOpen(v);
        if (v) setOpened(true);
      }}
    >
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
      {opened ? (
        <Suspense fallback={null}>
          <NotificationBellContent
            tab={tab}
            onTabChange={setTab}
            onClose={() => setOpen(false)}
            unreadCount={count}
          />
        </Suspense>
      ) : null}
    </DropdownMenu>
  );
}
