import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import { formatDistanceToNow } from "date-fns";

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

export const TASK_STATUSES = [
  "backlog",
  "todo",
  "in_progress",
  "resolved",
  "in_review",
  "closed",
] as const;
export type TaskStatus = (typeof TASK_STATUSES)[number];

export const STATUS_LABELS: Record<TaskStatus, string> = {
  backlog: "Backlog",
  todo: "Todo",
  in_progress: "In Progress",
  resolved: "Resolved",
  in_review: "In Review",
  closed: "Closed",
};

export const PRIORITIES = ["none", "low", "medium", "high", "urgent", "critical"] as const;
export type Priority = (typeof PRIORITIES)[number];

export const PRIORITY_LABELS: Record<Priority, string> = {
  none: "None",
  low: "Low",
  medium: "Medium",
  high: "High",
  urgent: "Urgent",
  critical: "Critical",
};

const ROLE_LABELS: Record<string, string> = {
  owner: "Owner",
  admin: "Admin",
  manager: "Manager",
  member: "Member",
  guest: "Guest",
};
export const roleLabel = (role: string): string => ROLE_LABELS[role] ?? role;

/** Format epoch-millisecond timestamps relative to now ("2h ago"). */
export function timeAgo(epochMs: number | null | undefined): string {
  if (!epochMs) return "—";
  try {
    return formatDistanceToNow(new Date(epochMs), { addSuffix: true });
  } catch {
    return "—";
  }
}

/**
 * Absolute rendering of an instant in a given IANA zone ("03/11/2026, 14:32").
 * Used for the hover title on relative timestamps so the profile timezone has
 * a concrete display effect. Falls back to the runtime default zone.
 */
export function formatInstant(epochMs: number | null | undefined, timeZone?: string | null): string {
  if (!epochMs) return "—";
  try {
    return new Intl.DateTimeFormat("en-GB", {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: timeZone || undefined,
    }).format(new Date(epochMs));
  } catch {
    // An invalid stored zone must never break rendering.
    try {
      return new Intl.DateTimeFormat("en-GB", { dateStyle: "medium", timeStyle: "short" }).format(
        new Date(epochMs),
      );
    } catch {
      return "—";
    }
  }
}

/** Deterministic pastel-free avatar background from any string. */
export function avatarTone(seed: string): string {
  let hash = 0;
  for (const ch of seed) {
    hash = (hash * 31 + ch.charCodeAt(0)) | 0;
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue} 55% 45%)`;
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).slice(0, 2);
  return parts.map((p) => p.charAt(0).toUpperCase()).join("") || "?";
}
