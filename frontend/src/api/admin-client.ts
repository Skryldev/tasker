// Instance-administration + picker API surface (Phase 12.5). Split from
// client.ts so the eager bundle carries none of it — the only consumers are
// lazy chunks (AdminUsersPage, AddMemberDialog, ContributionPicker).
import { request } from "./client";
import type { AdminUser } from "./client";

export interface AdminUsersResponse {
  items: AdminUser[];
  page: number;
  page_size: number;
  total: number;
}

export interface EligibleMember {
  id: string;
  email: string;
  display_name: string;
  username: string | null;
}

export interface TeamTaskSearchItem {
  id: string;
  title: string;
  status: string;
  project_id: string;
  project_name: string;
}

export const adminApi = {
  adminUsers(query: { q?: string; page?: number; page_size?: number } = {}): Promise<AdminUsersResponse> {
    return request("get", "/admin/users", { query }) as Promise<AdminUsersResponse>;
  },
  createAdminUser(body: {
    email: string;
    display_name: string;
    username?: string;
    temporary_password: string;
  }): Promise<AdminUser> {
    return request("post", "/admin/users", { body }) as Promise<AdminUser>;
  },
  updateAdminUser(
    userId: string,
    body: { display_name?: string; username?: string | null; active?: boolean },
  ): Promise<AdminUser> {
    return request("patch", `/admin/users/${userId}`, { body }) as Promise<AdminUser>;
  },
  /** No body: soft-deactivate. With confirmations: guarded hard delete. */
  deactivateAdminUser(userId: string): Promise<void> {
    return request("delete", `/admin/users/${userId}`) as Promise<void>;
  },
  deleteAdminUser(userId: string, confirmEmail: string, currentPassword: string): Promise<void> {
    return request("delete", `/admin/users/${userId}`, {
      body: { confirm_email: confirmEmail, current_password: currentPassword },
    }) as Promise<void>;
  },
  /** Team member assignment picker source (Bug 2): existing users only. */
  eligibleMembers(
    teamId: string,
    query: { q?: string; page?: number; page_size?: number } = {},
  ): Promise<{ items: EligibleMember[]; total: number }> {
    return request("get", `/teams/${teamId}/members/eligible`, { query }) as Promise<{
      items: EligibleMember[];
      total: number;
    }>;
  },
  /** Team-wide task search (Phase 12.5 Bug 6 pickers). */
  searchTeamTasks(
    teamId: string,
    query: { q?: string; page?: number; page_size?: number },
  ): Promise<{ items: TeamTaskSearchItem[]; page: number; page_size: number; total: number }> {
    return request("get", `/teams/${teamId}/tasks`, { query }) as Promise<{
      items: TeamTaskSearchItem[];
      page: number;
      page_size: number;
      total: number;
    }>;
  },
};
