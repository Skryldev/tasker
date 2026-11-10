import { useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Calendar,
  FileText,
  Globe,
  MessageSquare,
  ShieldCheck,
  X,
} from "lucide-react";
import { api, type GuestPermission, type TeamGuest2 } from "@/api/client";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { chipTextColor, chipBackground } from "@/lib/chip-color";
import { useToast } from "@/components/ui/toast";

/**
 * Guest permission drawer (Phase 13.5 Bug 10): a right-side drawer with
 * grouped toggles (Team / per-project access), batch-applied on save, and
 * color-coded permission badges on the guest card.
 */

type PermKey = GuestPermission["permission"];

/** Category identity per permission: color + icon + human label. */
const PERM_META: Record<
  PermKey,
  { label: string; description: string; icon: typeof Globe; color: string; group: "team" | "project" }
> = {
  view_team: {
    label: "See the team",
    description: "Team name and description appear in the switcher.",
    icon: Globe,
    color: "#64748b",
    group: "team",
  },
  view_project: {
    label: "See the project",
    description: "The project appears in this guest's Projects list.",
    icon: FileText,
    color: "#3b82f6",
    group: "project",
  },
  view_task: {
    label: "Open tasks",
    description: "Task titles and details are visible.",
    icon: FileText,
    color: "#10b981",
    group: "project",
  },
  comment_task: {
    label: "Comment on tasks",
    description: "The guest can post comments on tasks.",
    icon: MessageSquare,
    color: "#8b5cf6",
    group: "project",
  },
  view_activity: {
    label: "See task activity",
    description: "Non-sensitive workflow events are visible.",
    icon: Calendar,
    color: "#f59e0b",
    group: "project",
  },
};

export function PermissionBadge({
  permission,
  onRemove,
}: {
  permission: GuestPermission;
  onRemove?: () => void;
}) {
  const meta = PERM_META[permission.permission];
  const dark = document.documentElement.classList.contains("dark");
  const text = chipTextColor(meta.color, dark);
  const bg = chipBackground(meta.color, dark);
  return (
    <span
      className="inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-2xs font-medium"
      style={{ backgroundColor: bg, borderColor: `${meta.color}55`, color: text }}
      title={meta.description}
      data-testid={`perm-badge-${permission.id}`}
    >
      <meta.icon className="h-3 w-3" aria-hidden />
      <span className="truncate">{meta.label}</span>
      {onRemove ? (
        <button
          type="button"
          aria-label={`Remove permission ${meta.label}`}
          onClick={onRemove}
          className="ml-0.5 rounded-full hover:bg-black/10 dark:hover:bg-white/10"
          data-testid={`perm-remove-${permission.id}`}
        >
          <X className="h-3 w-3" />
        </button>
      ) : null}
    </span>
  );
}

export function GuestPermissionDrawer({
  teamId,
  guest,
  projects,
  onClose,
}: {
  teamId: string;
  guest: TeamGuest2;
  projects: { id: string; name: string }[];
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const { push } = useToast();

  // Desired state: permission → project_id (null for team scope).
  type Desired = { permission: PermKey; project_id: string | null };
  const [desired, setDesired] = useState<Desired[]>(() =>
    guest.permissions.map((p) => ({
      permission: p.permission,
      project_id: p.scope_type === "project" ? (p.scope_id ?? null) : null,
    })),
  );

  const has = (permission: PermKey, project_id: string | null): boolean =>
    desired.some((d) => d.permission === permission && d.project_id === project_id);

  const toggle = (permission: PermKey, project_id: string | null) => {
    setDesired((prev) => {
      const exists = prev.some((d) => d.permission === permission && d.project_id === project_id);
      if (exists) {
        return prev.filter((d) => !(d.permission === permission && d.project_id === project_id));
      }
      return [...prev, { permission, project_id }];
    });
  };

  const original = useMemo(
    () =>
      new Set(
        guest.permissions.map(
          (p) => `${p.permission}:${p.scope_type === "project" ? (p.scope_id ?? "") : ""}`,
        ),
      ),
    [guest],
  );

  const save = async () => {
    try {
      // Adds
      for (const d of desired) {
        const key = `${d.permission}:${d.project_id ?? ""}`;
        if (original.has(key)) continue;
        await api.grantGuestPermission(teamId, guest.guest_id, {
          permission: d.permission,
          scope_type: d.project_id === null ? "team" : "project",
          project_id: d.project_id ?? undefined,
        });
      }
      // Removes
      for (const p of guest.permissions) {
        const key = `${p.permission}:${p.scope_type === "project" ? (p.scope_id ?? "") : ""}`;
        if (desired.some((d) => `${d.permission}:${d.project_id ?? ""}` === key)) continue;
        await api.revokeGuestPermission(teamId, guest.guest_id, p.id);
      }
      push("success", "Permissions updated");
      void qc.invalidateQueries({ queryKey: ["team-guests2", teamId] });
      onClose();
    } catch (error) {
      push("error", "Could not save permissions", error instanceof Error ? error.message : undefined);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="fixed inset-0 z-50 flex" role="dialog" aria-modal aria-label={`Permissions for ${guest.user.display_name}`}>
      <button
        type="button"
        aria-label="Close permissions drawer"
        className="flex-1 bg-black/40"
        onClick={onClose}
      />
      <aside
        className="flex w-full max-w-md flex-col border-l bg-background shadow-xl"
        data-testid="permission-drawer"
      >
        <header className="flex items-center justify-between border-b px-4 py-3">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold">{guest.user.display_name}</h2>
            <p className="truncate text-2xs text-muted-foreground">{guest.user.email}</p>
          </div>
          <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
            <X />
          </Button>
        </header>

        <div className="flex flex-1 flex-col gap-5 overflow-y-auto scrollbar-thin p-4">
          {/* Team section */}
          <section aria-label="Team access">
            <h3 className="mb-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
              Team
            </h3>
            <PermToggle
              meta={PERM_META.view_team}
              checked={has("view_team", null)}
              onToggle={() => toggle("view_team", null)}
              testId={`perm-toggle-view_team`}
            />
          </section>

          {/* Project sections */}
          <section aria-label="Project access">
            <h3 className="mb-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
              Projects
            </h3>
            <div className="flex flex-col gap-3">
              {projects.map((p) => (
                <div key={p.id} className="rounded-lg border p-3">
                  <p className="mb-2 truncate text-xs font-medium">{p.name}</p>
                  <div className="flex flex-col gap-2">
                    {(["view_project", "view_task", "comment_task", "view_activity"] as PermKey[]).map(
                      (perm) => (
                        <PermToggle
                          key={perm}
                          meta={PERM_META[perm]}
                          checked={has(perm, p.id)}
                          onToggle={() => toggle(perm, p.id)}
                          testId={`perm-toggle-${p.id}-${perm}`}
                        />
                      ),
                    )}
                  </div>
                </div>
              ))}
            </div>
          </section>
        </div>

        <footer className="flex justify-end gap-2 border-t p-4">
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={() => void save()} data-testid="perm-save">
            <ShieldCheck /> Save permissions
          </Button>
        </footer>
      </aside>
    </div>
  );
}

function PermToggle({
  meta,
  checked,
  onToggle,
  testId,
}: {
  meta: { label: string; description: string; icon: typeof Globe };
  checked: boolean;
  onToggle: () => void;
  testId: string;
}) {
  return (
    <label
      className={cn(
        "flex cursor-pointer items-start gap-2.5 rounded-md p-2 transition-colors duration-100 hover:bg-accent/50",
        checked && "bg-primary/5",
      )}
      data-testid={testId}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={onToggle}
        className="mt-0.5 h-4 w-4 rounded border-input accent-primary"
      />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5 text-xs font-medium">
          <meta.icon className="h-3.5 w-3.5 text-muted-foreground" aria-hidden />
          {meta.label}
        </span>
        <span className="block text-2xs text-muted-foreground">{meta.description}</span>
      </span>
    </label>
  );
}
