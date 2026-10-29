import { useCallback } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./client";

export const notificationKeys = {
  list: (unread: boolean) => ["notifications", unread] as const,
  unreadCount: ["notifications", "unread-count"] as const,
};

const REFRESH_MS = 60_000;

export function useNotifications(unread: boolean, page = 1) {
  return useQuery({
    queryKey: notificationKeys.list(unread),
    queryFn: () => api.listNotifications(unread, page, 20),
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
  return { markRead, markAllRead };
}
