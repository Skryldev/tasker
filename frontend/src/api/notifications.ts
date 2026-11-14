import { useCallback } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./client";

export const notificationKeys = {
  list: (tab: string, page: number) => ["notifications", "tab", tab, page] as const,
  pinned: ["notifications", "pinned"] as const,
  counts: ["notifications", "counts"] as const,
  unreadCount: ["notifications", "unread-count"] as const,
};

const REFRESH_MS = 60_000;
const PAGE_SIZE = 50;

/** The action center tabs (Phase 14 WS5, ADR-0022). */
export type NotificationTab = "all" | "unread" | "mentions" | "assigned" | "activity";

export function eventTab(event: string): Exclude<NotificationTab, "all" | "unread"> {
  if (event === "comment_mention") return "mentions";
  if (event === "task_assigned") return "assigned";
  return "activity";
}

export function useNotifications(tab: NotificationTab, page = 1) {
  return useQuery({
    queryKey: notificationKeys.list(tab, page),
    queryFn: () => api.listNotifications(tab === "unread", page, PAGE_SIZE),
    refetchInterval: REFRESH_MS,
  });
}

export function usePinnedNotifications(enabled: boolean) {
  return useQuery({
    queryKey: notificationKeys.pinned,
    queryFn: () => api.listNotifications(false, 1, 50, true),
    enabled,
    refetchInterval: REFRESH_MS,
  });
}

export function useNotificationTabCounts(enabled: boolean) {
  return useQuery({
    queryKey: notificationKeys.counts,
    queryFn: api.notificationCounts,
    enabled,
    refetchInterval: REFRESH_MS,
  });
}

export function useUnreadCount() {
  return useQuery({
    queryKey: notificationKeys.unreadCount,
    queryFn: api.unreadCount,
    refetchInterval: REFRESH_MS,
  });
}

/** Mutations shared by the bell and panel; they refresh every notification view. */
export function useNotificationMutations() {
  const qc = useQueryClient();
  const invalidate = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["notifications"] });
  }, [qc]);

  const markRead = useMutation({
    mutationFn: (id: string) => api.markNotificationRead(id),
    onSuccess: invalidate,
  });
  const markAllRead = useMutation({
    mutationFn: () => api.markAllNotificationsRead(),
    onSuccess: invalidate,
  });
  const togglePin = useMutation({
    mutationFn: (vars: { id: string; pinned?: boolean }) =>
      api.toggleNotificationPin(vars.id, vars.pinned),
    onSuccess: invalidate,
  });
  return { markRead, markAllRead, togglePin };
}
