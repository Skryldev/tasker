import { IndicatorDot } from "@/components/ui/badge";
import { cn, PRIORITY_LABELS, STATUS_LABELS, type Priority, type TaskStatus } from "@/lib/utils";

const STATUS_DOT: Record<TaskStatus, string> = {
  backlog: "bg-status-backlog",
  todo: "bg-status-todo",
  in_progress: "bg-status-in-progress",
  in_review: "bg-status-in-review",
  resolved: "bg-status-resolved",
  closed: "bg-status-closed",
};

export function StatusBadge({ status, className }: { status: string; className?: string }) {
  const s = status as TaskStatus;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-0.5 text-2xs font-medium",
        className,
      )}
    >
      <IndicatorDot className={STATUS_DOT[s] ?? "bg-muted-foreground"} />
      {STATUS_LABELS[s] ?? status}
    </span>
  );
}

/** Three ascending bars; filled per priority level, pulsing for critical. */
export function PriorityBadge({ priority, className }: { priority: string; className?: string }) {
  const p = priority as Priority;
  const level: Record<Priority, number> = { none: 0, low: 1, medium: 2, high: 3, urgent: 3, critical: 3 };
  const color: Record<Priority, string> = {
    none: "text-muted-foreground",
    low: "text-priority-low",
    medium: "text-priority-medium",
    high: "text-priority-high",
    urgent: "text-priority-urgent",
    critical: "text-priority-urgent",
  };
  const filled = level[p] ?? 1;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 text-2xs font-medium",
        p === "critical" && "animate-pulse",
        className,
      )}
      title={`Priority: ${PRIORITY_LABELS[p] ?? priority}`}
    >
      <span className="flex items-end gap-px" aria-hidden>
        {[3, 5, 7].map((h, i) => (
          <span
            key={h}
            style={{ height: `${h}px` }}
            className={cn("w-0.5 rounded-sm", i < filled ? color[p] : "bg-border")}
          />
        ))}
      </span>
      {PRIORITY_LABELS[p] ?? priority}
    </span>
  );
}

export function StatusDot({ status, className }: { status: string; className?: string }) {
  return (
    <IndicatorDot
      className={cn(STATUS_DOT[status as TaskStatus] ?? "bg-muted-foreground", className)}
    />
  );
}

// ---------------------------------------------------------------------------
// Phase 12 WS5: label icons. The backend stores a whitelisted Lucide name;
// this map resolves the kebab-case name to the component. Names outside the
// whitelist never reach the client, so a miss renders nothing.
// ---------------------------------------------------------------------------

import {
  Bookmark,
  Box,
  Bug,
  Calendar,
  CircleAlert,
  CircleCheck,
  CircleX,
  Clock,
  Cloud,
  Compass,
  Cpu,
  Database,
  Eye,
  FileText,
  Flag,
  Flame,
  Folder,
  GitBranch,
  GitMerge,
  GitPullRequest,
  Globe,
  Heart,
  KeyRound,
  Layers,
  Lock,
  MapPin,
  MessageCircle,
  MessageSquare,
  Package,
  PenTool,
  Pencil,
  Rocket,
  Search,
  Server,
  Shield,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Star,
  Tag,
  Target,
  Terminal,
  TriangleAlert,
  Unlock,
  User,
  Users,
  Wrench,
  Zap,
  type LucideIcon,
} from "lucide-react";

export const LABEL_ICON_MAP: Record<string, LucideIcon> = {
  tag: Tag,
  bookmark: Bookmark,
  bug: Bug,
  wrench: Wrench,
  sparkles: Sparkles,
  rocket: Rocket,
  flame: Flame,
  zap: Zap,
  star: Star,
  heart: Heart,
  "circle-alert": CircleAlert,
  "triangle-alert": TriangleAlert,
  "circle-check": CircleCheck,
  "circle-x": CircleX,
  clock: Clock,
  calendar: Calendar,
  flag: Flag,
  target: Target,
  package: Package,
  box: Box,
  layers: Layers,
  folder: Folder,
  "file-text": FileText,
  pencil: Pencil,
  "pen-tool": PenTool,
  "message-circle": MessageCircle,
  "message-square": MessageSquare,
  users: Users,
  user: User,
  "git-branch": GitBranch,
  "git-merge": GitMerge,
  "git-pull-request": GitPullRequest,
  shield: Shield,
  "shield-alert": ShieldAlert,
  "shield-check": ShieldCheck,
  "key-round": KeyRound,
  lock: Lock,
  unlock: Unlock,
  eye: Eye,
  search: Search,
  compass: Compass,
  "map-pin": MapPin,
  globe: Globe,
  cloud: Cloud,
  database: Database,
  server: Server,
  cpu: Cpu,
  terminal: Terminal,
};

export function LabelIcon({ icon, className }: { icon: string; className?: string }) {
  const Component = LABEL_ICON_MAP[icon];
  if (!Component) return null;
  return <Component className={cn("h-3 w-3", className)} aria-hidden />;
}
