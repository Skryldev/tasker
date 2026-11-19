import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

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

/**
 * Status/priority labels moved to i18n (Phase 15): the maps below are the
 * ENGLISH source values kept for aria strings and test affordances; user-
 * visible rendering goes through `statusKey`/`priorityKey` + t().
 */
export const STATUS_LABELS: Record<TaskStatus, string> = {
  backlog: "Backlog",
  todo: "Todo",
  in_progress: "In Progress",
  resolved: "Resolved",
  in_review: "In Review",
  closed: "Closed",
};

export const statusKey = (status: string): string => `tasks:status.${status}`;
export const priorityKey = (priority: string): string => `tasks:priority.${priority}`;

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

/** Resolution labels (accepted/rejected) live in the tasks namespace. */
export const resolutionKey = (r: string): string => `tasks:resolution.${r}`;

/** Role -> i18n key (translated at the call site; Phase 15 i18n). */
export const roleLabelKey = (role: string): string =>
  `common:roles.${ROLE_KEYS[role] ?? role}`;

const ROLE_KEYS: Record<string, string> = {
  owner: "owner",
  admin: "admin",
  manager: "manager",
  member: "member",
  guest: "guest",
};

/** The active BCP-47 tag for Intl formatting (Phase 15, ADR-0024). */
export function activeIntlLocale(): string {
  const lang = document.documentElement.lang;
  return lang === "fa" ? "fa-IR" : lang === "fr" ? "fr-FR" : lang || "en";
}

/**
 * Locale-aware relative time: «۲ روز پیش» (fa-IR), «il y a 2 jours» (fr-FR),
 * "2 days ago" (en). Intl.RelativeTimeFormat — date-fns distance formatting
 * was English-only. Persian numerals come from the fa-IR locale itself.
 */
export function timeAgo(epochMs: number | null | undefined): string {
  if (!epochMs) return "—";
  const rtf = new Intl.RelativeTimeFormat(activeIntlLocale(), { numeric: "auto" });
  const diff = epochMs - Date.now();
  const abs = Math.abs(diff);
  const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ["year", 365 * 86_400_000],
    ["month", 30 * 86_400_000],
    ["week", 7 * 86_400_000],
    ["day", 86_400_000],
    ["hour", 3_600_000],
    ["minute", 60_000],
    ["second", 1000],
  ];
  try {
    for (const [unit, ms] of units) {
      if (abs >= ms || unit === "second") {
        return rtf.format(Math.round(diff / ms), unit);
      }
    }
    return rtf.format(0, "second");
  } catch {
    return "—";
  }
}

/**
 * Absolute rendering of an instant in a given IANA zone. Locale-aware:
 * fa renders Jalali (Intl 'persian' calendar — the wall calendar matches the
 * Phase 6 Doran picker), en/fr render Gregorian. Used for the hover title on
 * relative timestamps so the profile timezone has a concrete display effect.
 */
export function formatInstant(epochMs: number | null | undefined, timeZone?: string | null): string {
  if (!epochMs) return "—";
  const locale = activeIntlLocale();
  const calendar = locale === "fa-IR" ? "persian" : undefined;
  try {
    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: timeZone || undefined,
      calendar,
    }).format(new Date(epochMs));
  } catch {
    // An invalid stored zone must never break rendering.
    try {
      return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short", calendar }).format(
        new Date(epochMs),
      );
    } catch {
      return "—";
    }
  }
}

/**
 * Locale-aware number rendering: Persian digits for fa-IR counts, French
 * spacing/grouping for fr-FR, plain for en. Technical identifiers (task IDs,
 * API keys) stay Latin — callers must not route those through here.
 */
export function formatNumber(value: number): string {
  try {
    return new Intl.NumberFormat(activeIntlLocale()).format(value);
  } catch {
    return String(value);
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
