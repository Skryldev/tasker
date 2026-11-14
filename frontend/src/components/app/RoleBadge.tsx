import { Crown, Eye, Shield, User, UserCheck, Users, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Role badge (Phase 14 WS4): every member row renders its role as a compact
 * pill with a distinct color + icon — previously only the Owner showed one
 * (the badge was gated behind `role === "owner"`, and Admin/Manager rows
 * swapped it for the manage menu).
 */

export interface RoleMeta {
  label: string;
  icon: LucideIcon;
  /** Badge color identity (background tint + border + text source). */
  color: string;
}

export const ROLE_META = {
  owner: { label: "Owner", icon: Crown, color: "#f59e0b" },
  admin: { label: "Admin", icon: Shield, color: "#8b5cf6" },
  manager: { label: "Manager", icon: Users, color: "#3b82f6" },
  member: { label: "Member", icon: User, color: "#64748b" },
  viewer: { label: "Viewer", icon: Eye, color: "#94a3b8" },
  guest: { label: "Guest", icon: UserCheck, color: "#14b8a6" },
} satisfies Record<string, RoleMeta>;

export function RoleBadge({ role, className }: { role: string; className?: string }) {
  const meta: RoleMeta = ROLE_META[role as keyof typeof ROLE_META] ?? ROLE_META.member;
  return (
    <span
      className={cn(
        "inline-flex h-5 items-center gap-1 rounded-full border px-1.5 text-2xs font-medium",
        className,
      )}
      style={{
        backgroundColor: `${meta.color}1f`,
        borderColor: `${meta.color}55`,
        color: meta.color,
      }}
      aria-label={`Role: ${meta.label}`}
      data-testid={`role-badge-${role}`}
    >
      <meta.icon className="h-3 w-3" aria-hidden />
      {meta.label}
    </span>
  );
}
