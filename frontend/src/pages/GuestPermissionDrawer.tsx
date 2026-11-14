import { useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Activity,
  Calendar,
  ChevronDown,
  CircleUser,
  Columns3,
  Download,
  FileText,
  FolderOpen,
  GanttChart,
  Globe,
  History,
  ListTodo,
  MessageSquare,
  MessagesSquare,
  Paperclip,
  ScrollText,
  ShieldCheck,
  Upload,
  UserPlus,
  Users,
  X,
  type LucideIcon,
} from "lucide-react";
import { api, type GuestPermission, type GuestPermissionKey, type TeamGuest2 } from "@/api/client";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { chipTextColor, chipBackground } from "@/lib/chip-color";
import { useToast } from "@/components/ui/toast";

/**
 * Guest permission drawer (Phase 14 WS2, ADR-0021): a right-side drawer
 * with domain-grouped accordions (Team / Projects / Tasks /
 * Administration), per-project scoping, switch toggles with dependency
 * rules, a live grant summary, and a transactional save (applied changes
 * are rolled back if any step fails).
 */

export type PermDomain = "team" | "projects" | "tasks" | "admin";

/** Category identity per permission: color + icon + human label + domain. */
export const PERM_META: Record<
  GuestPermissionKey,
  {
    label: string;
    description: string;
    icon: LucideIcon;
    color: string;
    domain: PermDomain;
    teamScoped: boolean;
  }
> = {
  // Team domain (team-scoped).
  view_team: {
    label: "See the team",
    description: "Team name and description appear in the switcher.",
    icon: Globe,
    color: "#64748b",
    domain: "team",
    teamScoped: true,
  },
  view_team_members: {
    label: "See the roster",
    description: "Member names, roles and emails are visible.",
    icon: Users,
    color: "#0ea5e9",
    domain: "team",
    teamScoped: true,
  },
  view_team_activity: {
    label: "See team activity",
    description: "Team-level activity feeds are visible.",
    icon: Activity,
    color: "#f59e0b",
    domain: "team",
    teamScoped: true,
  },
  // Projects domain.
  view_project_list: {
    label: "Browse the project index",
    description: "The list of the team's projects is visible (names only).",
    icon: FolderOpen,
    color: "#3b82f6",
    domain: "projects",
    teamScoped: true,
  },
  view_project: {
    label: "Open the project",
    description: "Project details are visible.",
    icon: FileText,
    color: "#3b82f6",
    domain: "projects",
    teamScoped: false,
  },
  view_project_tasks: {
    label: "See the task list",
    description: "The project's task list is visible.",
    icon: ListTodo,
    color: "#10b981",
    domain: "projects",
    teamScoped: false,
  },
  view_project_board: {
    label: "See the board",
    description: "The kanban board is visible.",
    icon: Columns3,
    color: "#10b981",
    domain: "projects",
    teamScoped: false,
  },
  view_project_timeline: {
    label: "See the timeline",
    description: "The schedule timeline is visible.",
    icon: GanttChart,
    color: "#10b981",
    domain: "projects",
    teamScoped: false,
  },
  // Tasks domain (project-scoped).
  view_task: {
    label: "Open tasks",
    description: "Task titles and details are visible.",
    icon: FileText,
    color: "#10b981",
    domain: "tasks",
    teamScoped: false,
  },
  view_task_own: {
    label: "Open own tasks",
    description: "Only tasks assigned to this guest are visible.",
    icon: CircleUser,
    color: "#14b8a6",
    domain: "tasks",
    teamScoped: false,
  },
  view_task_dates: {
    label: "See due dates",
    description: "Due dates and deadlines are visible.",
    icon: Calendar,
    color: "#f59e0b",
    domain: "tasks",
    teamScoped: false,
  },
  view_task_activity: {
    label: "See task activity",
    description: "Non-sensitive workflow events are visible.",
    icon: History,
    color: "#f59e0b",
    domain: "tasks",
    teamScoped: false,
  },
  view_task_comments: {
    label: "Read the discussion",
    description: "Comments marked visible to guests are readable.",
    icon: MessagesSquare,
    color: "#8b5cf6",
    domain: "tasks",
    teamScoped: false,
  },
  comment_task: {
    label: "Comment on tasks",
    description: "The guest can post comments on tasks.",
    icon: MessageSquare,
    color: "#8b5cf6",
    domain: "tasks",
    teamScoped: false,
  },
  view_task_attachments: {
    label: "See the file list",
    description: "Attachment names and metadata are visible.",
    icon: Paperclip,
    color: "#ec4899",
    domain: "tasks",
    teamScoped: false,
  },
  download_task_attachments: {
    label: "Download files",
    description: "Attachment contents can be downloaded.",
    icon: Download,
    color: "#ec4899",
    domain: "tasks",
    teamScoped: false,
  },
  upload_task_attachments: {
    label: "Attach files",
    description: "The guest can upload attachments (implies commenting).",
    icon: Upload,
    color: "#ec4899",
    domain: "tasks",
    teamScoped: false,
  },
  // Administration domain (team-scoped).
  invite_guest: {
    label: "Request guest invitations",
    description: "The guest can ask you to invite someone; you approve or reject.",
    icon: UserPlus,
    color: "#8b5cf6",
    domain: "admin",
    teamScoped: true,
  },
  view_audit_log: {
    label: "Read the audit log",
    description: "The team's audit trail is visible.",
    icon: ScrollText,
    color: "#64748b",
    domain: "admin",
    teamScoped: true,
  },
};

export const DOMAIN_LABELS: Record<PermDomain, string> = {
  team: "Team",
  projects: "Projects",
  tasks: "Tasks",
  admin: "Administration",
};

export function PermissionBadge({
  permission,
  onRemove,
}: {
  permission: GuestPermission;
  onRemove?: () => void;
}) {
  const meta =
    PERM_META[permission.permission as GuestPermissionKey] ?? PERM_META.view_team;
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

/** Accessible switch toggle (no extra dependency — role="switch" button). */
function Switch({
  checked,
  disabled,
  onToggle,
  label,
  title,
  testId,
}: {
  checked: boolean;
  disabled?: boolean;
  onToggle: () => void;
  label: string;
  title?: string;
  testId: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
      className={cn(
        "relative mt-0.5 h-4.5 w-8 shrink-0 rounded-full border transition-colors duration-150",
        checked ? "border-primary bg-primary" : "bg-input",
        disabled && "cursor-not-allowed opacity-40",
      )}
      data-testid={testId}
      data-state={checked ? "on" : "off"}
    >
      <span
        className={cn(
          "absolute top-1/2 block h-3.5 w-3.5 -translate-y-1/2 rounded-full bg-background shadow transition-[left] duration-150",
          checked ? "left-[15px]" : "left-[2px]",
        )}
      />
    </button>
  );
}

/** One accordion section (button + chevron; content rendered when open). */
function DrawerSection({
  label,
  count,
  open,
  onToggleOpen,
  children,
}: {
  label: string;
  count: number;
  open: boolean;
  onToggleOpen: () => void;
  children: React.ReactNode;
}) {
  return (
    <section aria-label={`${DOMAIN_LABELS[label as PermDomain] ?? label} permissions`}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggleOpen}
        className="flex w-full items-center justify-between rounded-md px-1 py-1.5 text-left hover:bg-accent/50"
        data-testid={`perm-section-${label}`}
      >
        <span className="flex items-center gap-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
          <ChevronDown
            className={cn("h-3.5 w-3.5 transition-transform duration-150", !open && "-rotate-90")}
            aria-hidden
          />
          {DOMAIN_LABELS[label as PermDomain] ?? label}
        </span>
        {count > 0 ? (
          <span className="rounded-full bg-muted px-1.5 py-0.5 text-2xs font-medium text-muted-foreground">
            {count}
          </span>
        ) : null}
      </button>
      {open ? <div className="flex flex-col gap-3 pb-2 pt-1">{children}</div> : null}
    </section>
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
  type Desired = { permission: GuestPermissionKey; project_id: string | null };
  const [desired, setDesired] = useState<Desired[]>(() =>
    guest.permissions.map((p) => ({
      permission: p.permission as GuestPermissionKey,
      project_id: p.scope_type === "project" ? (p.scope_id ?? null) : null,
    })),
  );
  const [openSections, setOpenSections] = useState<Record<PermDomain, boolean>>({
    team: true,
    projects: true,
    tasks: true,
    admin: false,
  });
  const [saving, setSaving] = useState(false);

  const has = (permission: GuestPermissionKey, project_id: string | null): boolean =>
    desired.some((d) => d.permission === permission && d.project_id === project_id);

  const toggle = (permission: GuestPermissionKey, project_id: string | null) => {
    setDesired((prev) => {
      const exists = prev.some((d) => d.permission === permission && d.project_id === project_id);
      let next: Desired[];
      if (exists) {
        next = prev.filter((d) => !(d.permission === permission && d.project_id === project_id));
      } else {
        next = [...prev, { permission, project_id }];
      }
      // Dependency rules (ADR-0021):
      //   * view_task and view_task_own are mutually exclusive — view_task
      //     wins, so checking it clears view_task_own.
      //   * upload_task_attachments implies commenting, so checking it
      //     auto-enables comment_task.
      if (!exists && permission === "view_task") {
        next = next.filter(
          (d) => !(d.permission === "view_task_own" && d.project_id === project_id),
        );
      }
      if (!exists && permission === "upload_task_attachments") {
        if (!next.some((d) => d.permission === "comment_task" && d.project_id === project_id)) {
          next.push({ permission: "comment_task", project_id });
        }
      }
      return next;
    });
  };

  const original = useMemo(
    () =>
      new Set(
        guest.permissions.map(
          (p) =>
            `${p.permission as GuestPermissionKey}:${p.scope_type === "project" ? (p.scope_id ?? "") : ""}`,
        ),
      ),
    [guest],
  );

  const grantCount = desired.length;

  const save = async () => {
    // Transactional save (ADR-0021): compute the diff first, then apply.
    // If any step fails, the already-applied operations are rolled back
    // (compensated) in reverse so the stored set matches the pre-save state.
    const adds = desired.filter(
      (d) => !original.has(`${d.permission}:${d.project_id ?? ""}`),
    );
    const removes = guest.permissions.filter((p) => {
      const key = `${p.permission as GuestPermissionKey}:${p.scope_type === "project" ? (p.scope_id ?? "") : ""}`;
      return !desired.some((d) => `${d.permission}:${d.project_id ?? ""}` === key);
    });
    if (adds.length === 0 && removes.length === 0) {
      onClose();
      return;
    }
    setSaving(true);
    const applied: Array<() => Promise<unknown>> = [];
    try {
      for (const d of adds) {
        await api.grantGuestPermission(teamId, guest.guest_id, {
          permission: d.permission,
          scope_type: d.project_id === null ? "team" : "project",
          project_id: d.project_id ?? undefined,
        });
        applied.push(() =>
          // Compensation: revoke the just-added grant.
          (async () => {
            const { items } = await api.listGuestPermissions(teamId, guest.guest_id);
            const row = items.find(
              (i) =>
                i.permission === d.permission &&
                (i.scope_type === "project" ? i.scope_id : null) === d.project_id,
            );
            if (row) await api.revokeGuestPermission(teamId, guest.guest_id, row.id);
          })(),
        );
      }
      for (const p of removes) {
        await api.revokeGuestPermission(teamId, guest.guest_id, p.id);
        applied.push(() =>
          // Compensation: re-grant the just-removed permission.
          api.grantGuestPermission(teamId, guest.guest_id, {
            permission: p.permission,
            scope_type: p.scope_type,
            project_id: p.scope_type === "project" ? (p.scope_id ?? undefined) : undefined,
          }),
        );
      }
      push("success", "Permissions updated");
      void qc.invalidateQueries({ queryKey: ["team-guests2", teamId] });
      onClose();
    } catch (error) {
      // Roll back the applied operations in reverse.
      for (const undo of applied.reverse()) {
        try {
          await undo();
        } catch {
          // Best-effort: a failed compensation leaves the drawer's diff
          // available for a retry.
        }
      }
      void qc.invalidateQueries({ queryKey: ["team-guests2", teamId] });
      push(
        "error",
        "Could not save permissions — changes rolled back",
        error instanceof Error ? error.message : undefined,
      );
    } finally {
      setSaving(false);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const teamGrants = desired.filter((d) => PERM_META[d.permission]?.teamScoped).length;
  const countIn = (domain: PermDomain) =>
    desired.filter((d) => PERM_META[d.permission]?.domain === domain).length;

  return (
    <div
      className="fixed inset-0 z-50 flex"
      role="dialog"
      aria-modal
      aria-label={`Permissions for ${guest.user.display_name}`}
    >
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
          <div className="flex items-center gap-2">
            <span
              className="rounded-full border px-2 py-0.5 text-2xs font-medium text-muted-foreground"
              data-testid="perm-summary"
              aria-label={`${grantCount} grants active`}
            >
              {grantCount} {grantCount === 1 ? "grant" : "grants"}
            </span>
            <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
              <X />
            </Button>
          </div>
        </header>

        <div className="flex flex-1 flex-col gap-2 overflow-y-auto scrollbar-thin p-4">
          {/* Team domain */}
          <DrawerSection
            label="team"
            count={countIn("team")}
            open={openSections.team}
            onToggleOpen={() => setOpenSections((s) => ({ ...s, team: !s.team }))}
          >
            {(
              [
                "view_team",
                "view_team_members",
                "view_team_activity",
              ] as GuestPermissionKey[]
            ).map((perm) => (
              <PermRow
                key={perm}
                perm={perm}
                checked={has(perm, null)}
                onToggle={() => toggle(perm, null)}
                testId={`perm-toggle-${perm}`}
              />
            ))}
          </DrawerSection>

          {/* Projects domain */}
          <DrawerSection
            label="projects"
            count={countIn("projects")}
            open={openSections.projects}
            onToggleOpen={() => setOpenSections((s) => ({ ...s, projects: !s.projects }))}
          >
            <PermRow
              perm="view_project_list"
              checked={has("view_project_list", null)}
              onToggle={() => toggle("view_project_list", null)}
              testId="perm-toggle-view_project_list"
            />
            {projects.map((p) => (
              <div key={p.id} className="rounded-lg border p-3">
                <p className="mb-2 truncate text-xs font-medium">{p.name}</p>
                <div className="flex flex-col gap-2">
                  {(
                    [
                      "view_project",
                      "view_project_tasks",
                      "view_project_board",
                      "view_project_timeline",
                    ] as GuestPermissionKey[]
                  ).map((perm) => (
                    <PermRow
                      key={perm}
                      perm={perm}
                      checked={has(perm, p.id)}
                      onToggle={() => toggle(perm, p.id)}
                      testId={`perm-toggle-${p.id}-${perm}`}
                    />
                  ))}
                </div>
              </div>
            ))}
          </DrawerSection>

          {/* Tasks domain */}
          <DrawerSection
            label="tasks"
            count={countIn("tasks")}
            open={openSections.tasks}
            onToggleOpen={() => setOpenSections((s) => ({ ...s, tasks: !s.tasks }))}
          >
            {projects.map((p) => (
              <div key={p.id} className="rounded-lg border p-3">
                <p className="mb-2 truncate text-xs font-medium">{p.name}</p>
                <div className="flex flex-col gap-2">
                  {(
                    [
                      "view_task",
                      "view_task_own",
                      "view_task_dates",
                      "view_task_activity",
                      "view_task_comments",
                      "comment_task",
                      "view_task_attachments",
                      "download_task_attachments",
                      "upload_task_attachments",
                    ] as GuestPermissionKey[]
                  ).map((perm) => {
                    // view_task and view_task_own are mutually exclusive;
                    // view_task wins (ADR-0021).
                    const blockedByViewTask =
                      perm === "view_task_own" && has("view_task", p.id);
                    return (
                      <PermRow
                        key={perm}
                        perm={perm}
                        checked={has(perm, p.id)}
                        disabled={blockedByViewTask}
                        blockedNote={
                          blockedByViewTask
                            ? "Covered by “Open tasks” — view_task already includes own tasks."
                            : undefined
                        }
                        onToggle={() => toggle(perm, p.id)}
                        testId={`perm-toggle-${p.id}-${perm}`}
                      />
                    );
                  })}
                </div>
              </div>
            ))}
          </DrawerSection>

          {/* Administration domain */}
          <DrawerSection
            label="admin"
            count={countIn("admin")}
            open={openSections.admin}
            onToggleOpen={() => setOpenSections((s) => ({ ...s, admin: !s.admin }))}
          >
            {(["invite_guest", "view_audit_log"] as GuestPermissionKey[]).map((perm) => (
              <PermRow
                key={perm}
                perm={perm}
                checked={has(perm, null)}
                onToggle={() => toggle(perm, null)}
                testId={`perm-toggle-${perm}`}
              />
            ))}
          </DrawerSection>

          <p className="text-2xs leading-relaxed text-muted-foreground">
            {teamGrants} team-scoped {teamGrants === 1 ? "grant" : "grants"}; the rest are scoped
            per project. Saving applies every add and removal in one pass — a failed step rolls the
            batch back.
          </p>
        </div>

        <footer className="flex justify-end gap-2 border-t p-4">
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={() => void save()} disabled={saving} data-testid="perm-save">
            <ShieldCheck /> Save permissions
          </Button>
        </footer>
      </aside>
    </div>
  );
}

function PermRow({
  perm,
  checked,
  disabled,
  blockedNote,
  onToggle,
  testId,
}: {
  perm: GuestPermissionKey;
  checked: boolean;
  disabled?: boolean;
  blockedNote?: string;
  onToggle: () => void;
  testId: string;
}) {
  const meta = PERM_META[perm];
  return (
    <div
      className={cn(
        "flex cursor-pointer items-start gap-2.5 rounded-md p-2 transition-colors duration-100 hover:bg-accent/50",
        checked && "bg-primary/5",
        disabled && "pointer-events-none opacity-60",
      )}
      onClick={disabled ? undefined : onToggle}
      data-testid={testId}
      data-permission={perm}
    >
      <Switch
        checked={checked}
        disabled={disabled}
        onToggle={onToggle}
        label={meta.label}
        title={blockedNote ?? meta.description}
        testId={`${testId}-switch`}
      />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5 text-xs font-medium">
          <meta.icon className="h-3.5 w-3.5 text-muted-foreground" aria-hidden />
          {meta.label}
        </span>
        <span className="block text-2xs text-muted-foreground">
          {blockedNote ?? meta.description}
        </span>
      </span>
    </div>
  );
}
