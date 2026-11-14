import { Ban, ArrowUpNarrowWide, Equal, ArrowUpWideNarrow, Flame, Siren, type LucideIcon } from "lucide-react";
import type { Priority } from "./utils";

/**
 * Priority guidance (Phase 14 WS3): bilingual (Persian/English) descriptions
 * and per-level icon/color so every priority picker teaches the scale.
 */
export interface PriorityMeta {
  icon: LucideIcon;
  color: string;
  fa: string;
  en: string;
}

export const PRIORITY_META: Record<Priority, PriorityMeta> = {
  none: {
    icon: Ban,
    color: "text-muted-foreground",
    fa: "بدون اولویت — تسک‌های معمولی یا بک‌لاگ",
    en: "No priority — normal or backlog tasks",
  },
  low: {
    icon: ArrowUpNarrowWide,
    color: "text-priority-low",
    fa: "اهمیت کم — می‌تواند منتظر بماند",
    en: "Low importance — can wait",
  },
  medium: {
    icon: Equal,
    color: "text-priority-medium",
    fa: "اهمیت معمولی — در روند عادی کار",
    en: "Normal importance — in the regular flow",
  },
  high: {
    icon: ArrowUpWideNarrow,
    color: "text-priority-high",
    fa: "مهم — باید در اسپرینت فعلی انجام شود",
    en: "Important — should be done in the current sprint",
  },
  urgent: {
    icon: Flame,
    color: "text-priority-urgent",
    fa: "فوری — مانع پیشرفت کار تیم شده است",
    en: "Urgent — blocking the team",
  },
  critical: {
    icon: Siren,
    color: "text-priority-urgent",
    fa: "بحرانی — نیاز به رسیدگی فوری و مداخله مدیریت",
    en: "Critical — requires immediate attention and management intervention",
  },
};
