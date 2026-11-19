import { useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useConfirmDialog } from "@/components/ui/confirm-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  MoreHorizontal,
  Search,
  ShieldAlert,
  Tags,
  Trash2,
  UserPlus,
  Users,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { api, ApiError, type Label as LabelData, type Member, type TrashProject } from "@/api/client";
import {
  useLabels,
  useMembers,
  useTeam,
  useTeamGuests,
  useRevokeTeamGuest,
  useArchiveTeam,
  useUnarchiveTeam,
} from "@/api/hooks";
import { Avatar, Separator } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Badge, Skeleton } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input, Label } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { EmptyState } from "@/components/app/EmptyState";
import { TimezoneSelect, timezoneValue } from "@/components/app/TimezoneSelect";
import { useToast } from "@/components/ui/toast";
import { useWorkspace, useIsAdmin } from "@/lib/workspace";
import { fileToAvatarDataUrl, AvatarError } from "@/lib/avatar";
import { setTeamAvatarVersion, teamAvatarUrl } from "@/lib/avatar-version";
import { cn, roleLabelKey, timeAgo } from "@/lib/utils";
import { LABEL_PALETTE } from "@/lib/label-palette";

const ROLES = ["owner", "admin", "manager", "member"] as const;

export function TeamSettingsPage() {
  const { t } = useTranslation("settings");
  const { teamId } = useWorkspace();
  const team = useTeam(teamId);
  // Guest permission scopes come from the team's projects (Phase 13).
  const projectsScope = useQuery({
    queryKey: ["projects", teamId],
    queryFn: () => api.listProjects(teamId!),
    enabled: Boolean(teamId),
  });
  const projectsForScope = (projectsScope.data?.items ?? []).map((p) => ({
    id: p.id,
    name: p.name,
  }));
  const isAdmin = useIsAdmin();
  const archived = Boolean(team.data?.archived_at);

  if (!teamId || team.isError) {
    return (
      <EmptyState
        icon={<Users className="h-8 w-8" />}
        title={t("team.unavailable")}
        description={t("team.unavailableHint")}
      />
    );
  }

  return (
    <div className="mx-auto max-w-3xl px-6 pb-4 pt-6 [&>*:last-child]:mb-0">
      <header className="mb-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            {t("team.settingsTitle", { name: team.data?.name ?? t("team.fallbackName") })}
            {archived ? (
              <Badge variant="muted" data-testid="team-archived-badge">
                {t("team.archived")}
              </Badge>
            ) : null}
          </h1>
          <p className="text-xs text-muted-foreground">
            {team.data?.description || t("team.manageHint")}
          </p>
        </div>
      </header>

      {isAdmin && team.data ? (
        <>
          <section aria-label={t("team.details")} className="mb-8">
            <h2 className="mb-3 text-sm font-semibold">{t("team.details")}</h2>
            <TeamDetailsForm
              teamId={teamId}
              initialName={team.data.name}
              initialDescription={team.data.description ?? ""}
              initialTimezone={team.data.timezone ?? ""}
            />
          </section>
          <Separator className="mb-8" />
        </>
      ) : null}

      <MembersSection teamId={teamId} isAdmin={isAdmin} />
      <Separator className="my-8" />
      <GuestsSection teamId={teamId} canManage={isAdmin} />
      <TeamGuestsSection teamId={teamId} projects={projectsForScope} canManage={isAdmin} />
      <Separator className="my-8" />
      <LabelsSection teamId={teamId} isAdmin={isAdmin} />
      {isAdmin ? (
        <>
          <Separator className="my-8" />
          <ProjectTrashSection teamId={teamId} />
        </>
      ) : null}
      {isAdmin ? (
        <>
          <Separator className="my-8" />
          <DangerZoneSection teamId={teamId} teamName={team.data?.name ?? ""} archived={archived} />
        </>
      ) : null}
      {/* Trailing spacer eats escaped margins so the scroll height is
          content-driven (Phase 13.5 Bug 3). */}
      <div aria-hidden className="h-0 overflow-hidden" data-testid="settings-tail" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------

function TeamDetailsForm({
  teamId,
  initialName,
  initialDescription,
  initialTimezone,
}: {
  teamId: string;
  initialName: string;
  initialDescription: string;
  initialTimezone: string;
}) {
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const [timezone, setTimezone] = useState(initialTimezone || "none");
  const { t } = useTranslation("settings");
  const { push } = useToast();
  const qc = useQueryClient();
  const fileInput = useRef<HTMLInputElement>(null);
  const [localError, setLocalError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: () =>
      api.updateTeam(teamId, {
        name,
        description,
        timezone: timezoneValue(timezone) ?? undefined,
      }),
    onSuccess: () => {
      push("success", t("team.updated"));
      void qc.invalidateQueries({ queryKey: ["teams"] });
      void qc.invalidateQueries({ queryKey: ["team", teamId] });
    },
    onError: (error) => push("error", t("team.errors.update"), error.message),
  });

  const onPickFile = async (file: File | undefined) => {
    if (!file) return;
    setLocalError(null);
    try {
      const dataUrl = await fileToAvatarDataUrl(file);
      api
        .updateTeam(teamId, { avatar: dataUrl })
        .then(() => {
          push("success", t("team.avatarUpdated"));
          void qc.invalidateQueries({ queryKey: ["team", teamId] });
          // Bug 8: cache-bust the team avatar across every surface (same
          // pattern as the profile avatar, Phase 12.5).
          setTeamAvatarVersion(teamId, Date.now());
        })
        .catch((error) =>
          setLocalError(
            error instanceof ApiError ? error.message : t("avatar.errors.upload"),
          ),
        );
    } catch (error) {
      setLocalError(error instanceof AvatarError ? error.message : t("avatar.errors.read"));
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate();
        }}
      >
        <div className="flex items-center gap-4">
          <Avatar
            name={name || "?"}
            size="md"
            className="h-14 w-14 rounded-sg"
            src={teamAvatarUrl(teamId)}
          />
          <div className="flex flex-col gap-1">
            <Label htmlFor="team-avatar-upload" className="text-xs">
              {t("team.avatarLabel")}
            </Label>
            <input
              id="team-avatar-upload"
              ref={fileInput}
              type="file"
              accept="image/jpeg,image/png,image/webp"
              className="hidden"
              onChange={(e) => {
                void onPickFile(e.target.files?.[0]);
                e.target.value = "";
              }}
            />
            <div className="flex gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => fileInput.current?.click()}
                data-testid="team-avatar-pick"
              >
                {t("actions.upload")}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() =>
                  api
                    .updateTeam(teamId, { avatar: null })
                    .then(() => {
                      push("success", t("team.avatarRemoved"));
                      void qc.invalidateQueries({ queryKey: ["team", teamId] });
                    })
                    .catch(() => push("error", t("team.errors.avatarRemove")))
                }
              >
                {t("actions.remove")}
              </Button>
            </div>
          </div>
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="team-name">{t("team.name")}</Label>
          <Input id="team-name" value={name} onChange={(e) => setName(e.target.value)} className="max-w-sm" />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="team-description">{t("team.description")}</Label>
          <Input
            id="team-description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className="max-w-sm"
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="team-timezone">{t("team.timezone")}</Label>
          <div className="max-w-sm" data-testid="team-timezone-select">
            <TimezoneSelect
              id="team-timezone"
              value={timezone}
              onChange={setTimezone}
            />
          </div>
        </div>
        {localError ? (
          <p className="text-2xs text-destructive" role="alert">{localError}</p>
        ) : null}
        <Button type="submit" size="sm" className="self-start" disabled={save.isPending || !name.trim()}>
          {save.isPending ? t("actions.saving") : t("team.save")}
        </Button>
      </form>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

function MembersSection({ teamId, isAdmin }: { teamId: string; isAdmin: boolean }) {
  const { t } = useTranslation();
  const members = useMembers(teamId);
  const { push } = useToast();
  const qc = useQueryClient();
  const { confirm, element: confirmElement } = useConfirmDialog();
  const [addOpen, setAddOpen] = useState(false);
  const [roleFilter, setRoleFilter] = useState("all");
  const [search, setSearch] = useState("");

  const invalidate = () => void qc.invalidateQueries({ queryKey: ["members", teamId] });

  const changeRole = useMutation({
    mutationFn: ({ userId, role }: { userId: string; role: string }) =>
      api.changeMemberRole(teamId, userId, role),
    onSuccess: () => {
      push("success", t("settings:members.roleUpdated"));
      invalidate();
    },
    onError: (error) =>
      push("error", t("settings:members.errors.roleChange"), error instanceof ApiError ? error.message : error.message),
  });

  const removeMember = useMutation({
    mutationFn: (userId: string) => api.removeMember(teamId, userId),
    onSuccess: () => {
      push("info", t("settings:members.removed"));
      invalidate();
    },
    onError: (error) =>
      push("error", t("settings:members.errors.remove"), error instanceof ApiError ? error.message : error.message),
  });

  const confirmRemove = (member: Member) => {
    confirm({
      title: t("settings:members.removeConfirm.title"),
      description: t("settings:members.removeConfirm.description", { name: member.user.display_name }),
      confirmLabel: t("settings:members.removeConfirm.confirm"),
      action: () => removeMember.mutate(member.user.id),
    });
  };

  const visible = useMemo(() => {
    const all = members.data?.items ?? [];
    const needle = search.trim().toLowerCase();
    return all.filter((member) => {
      if (roleFilter !== "all" && member.role !== roleFilter) return false;
      if (!needle) return true;
      return (
        member.user.display_name.toLowerCase().includes(needle) ||
        member.user.email.toLowerCase().includes(needle) ||
        (member.username ?? "").toLowerCase().includes(needle)
      );
    });
  }, [members.data, roleFilter, search]);

  return (
    <section aria-label={t("settings:members.sectionAria")}>
      {confirmElement}
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">{t("settings:members.title", { total: visible.length })}</h2>
        {isAdmin ? (
          <Button size="sm" variant="outline" onClick={() => setAddOpen(true)} data-testid="add-member">
            <UserPlus /> {t("settings:members.add")}
          </Button>
        ) : null}
      </div>
      <div className="mb-3 flex flex-wrap items-center gap-2" data-testid="member-filters">
        <div className="relative">
          <Search className="pointer-events-none absolute start-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder={t("settings:members.searchPlaceholder")}
            className="w-56 ps-7"
            aria-label={t("settings:members.searchAria")}
            dir="auto"
            data-testid="member-search"
          />
        </div>
        <Select value={roleFilter} onValueChange={setRoleFilter}>
          <SelectTrigger className="w-auto min-w-[8.5rem]" aria-label={t("settings:members.filterByRole")} data-testid="member-role-filter">
            <SelectValue placeholder={t("settings:members.role")} />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("settings:members.allRoles")}</SelectItem>
            {ROLES.map((r) => (
              <SelectItem key={r} value={r}>
                {t(roleLabelKey(r))}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {members.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : visible.length === 0 ? (
        <p className="text-xs text-muted-foreground" data-testid="members-empty">
          {t("settings:members.empty")}
        </p>
      ) : (
        <ul className="overflow-hidden rounded-sg border" data-testid="member-list">
          {visible.map((member) => (
            <li key={member.user.id} className="flex items-center gap-3 border-b px-4 py-2.5 last:border-0">
              <Avatar name={member.user.display_name} size="md" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">
                  {member.user.display_name}
                  {member.username ? (
                    <span className="ms-1.5 font-mono text-2xs text-muted-foreground">
                      @{member.username}
                    </span>
                  ) : null}
                  <RoleBadge role={member.role} className="ms-2" />
                </p>
                <p className="truncate text-2xs text-muted-foreground">
                  {t("settings:members.joinedAgo", { email: member.user.email, time: timeAgo(member.created_at) })}
                </p>
              </div>
              {isAdmin ? (
                <MemberRowMenu
                  member={member}
                  changeRole={changeRole}
                  confirmRemove={confirmRemove}
                />
              ) : null}
            </li>
          ))}
        </ul>
      )}
      {addOpen ? <AddMemberDialog open={addOpen} onOpenChange={setAddOpen} teamId={teamId} /> : null}
    </section>
  );
}

function MemberRowMenu({
  member,
  changeRole,
  confirmRemove,
}: {
  member: Member;
  changeRole: { mutate: (vars: { userId: string; role: string }) => void };
  confirmRemove: (member: Member) => void;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={t("settings:members.manageAria", { name: member.user.display_name })}>
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {ROLES.filter((r) => r !== member.role).map((role) => (
          <DropdownMenuItem
            key={role}
            onSelect={() => changeRole.mutate({ userId: member.user.id, role })}
          >
            {t("settings:members.makeRole", { role: t(roleLabelKey(role)) })}
          </DropdownMenuItem>
        ))}
        <DropdownMenuItem destructive onSelect={() => confirmRemove(member)}>
          <Trash2 /> {t("settings:members.removeFromTeam")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// Bug 2 (ADR-0014): the add-member flow picks an EXISTING instance user —
// account creation moved to the Owner-only Users page.
import { AddMemberDialog } from "@/pages/AddMemberDialog";
import { TeamGuestsSection } from "@/pages/TeamGuestsSection";
import { LabelChip } from "@/components/app/LabelChip";
import { RoleBadge } from "@/components/app/RoleBadge";
import { LABEL_ICON_MAP } from "@/components/app/TaskBadges";

// ---------------------------------------------------------------------------
// Guests (team-wide project viewers)
// ---------------------------------------------------------------------------

function GuestsSection({ teamId, canManage }: { teamId: string; canManage: boolean }) {
  const { t } = useTranslation("settings");
  const guests = useTeamGuests(teamId);
  const revoke = useRevokeTeamGuest(teamId);
  const { push } = useToast();
  const { confirm, element: confirmElement } = useConfirmDialog();

  const confirmRevoke = (userName: string, userId: string) => {
    confirm({
      title: t("guests.revokeConfirm.title"),
      description: t("guests.revokeConfirm.viewerDescription", { name: userName }),
      confirmLabel: t("guests.revokeConfirm.confirm"),
      action: () =>
        revoke.mutate(userId, {
          onSuccess: () => push("success", t("guests.accessRevoked")),
          onError: (error) =>
            push(
              "error",
              t("guests.errors.revoke"),
              error instanceof ApiError ? error.message : undefined,
            ),
        }),
    });
  };

  return (
    <section aria-label={t("guests.title")}>
      {confirmElement}
      <h2 className="mb-1 text-sm font-semibold">{t("guests.title")}</h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("guests.subtitle")}
      </p>
      {guests.isLoading ? (
        <Skeleton className="h-16 w-full" />
      ) : (guests.data?.items.length ?? 0) === 0 ? (
        <p className="text-xs text-muted-foreground" data-testid="guests-empty">
          {t("guests.empty")}
        </p>
      ) : (
        <ul className="overflow-hidden rounded-sg border" data-testid="guest-list">
          {guests.data?.items.map((guest) => (
            <li
              key={`${guest.project_id}-${guest.user.id}`}
              className="flex items-center gap-3 border-b px-4 py-2.5 last:border-0"
            >
              <Avatar name={guest.user.display_name} size="md" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">
                  {guest.user.display_name}
                  <span className="ms-1.5 truncate text-2xs text-muted-foreground">
                    {guest.user.email}
                  </span>
                </p>
                <p className="truncate text-2xs text-muted-foreground">
                  {t("guests.viewerOf")}{" "}
                  <span className="font-medium">{guest.project_name}</span> ·{" "}
                  {t("guests.grantedAgo", { time: timeAgo(guest.created_at) })}
                </p>
              </div>
              {canManage ? (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={revoke.isPending}
                  onClick={() => confirmRevoke(guest.user.display_name, guest.user.id)}
                  data-testid={`revoke-guest-${guest.user.id}`}
                >
                  {t("actions.revoke")}
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Danger zone
// ---------------------------------------------------------------------------

function DangerZoneSection({
  teamId,
  teamName,
  archived,
}: {
  teamId: string;
  teamName: string;
  archived: boolean;
}) {
  const { myRole } = useWorkspace();
  const { t } = useTranslation("settings");
  const { push } = useToast();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const archiveTeam = useArchiveTeam(teamId);
  const unarchiveTeam = useUnarchiveTeam(teamId);
  const isOwner = myRole === "owner";
  const [confirmName, setConfirmName] = useState("");
  const [deletePassword, setDeletePassword] = useState("");
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);

  const doArchive = () => {
    (archived ? unarchiveTeam : archiveTeam).mutate(undefined, {
      onSuccess: () => push("success", archived ? t("dangerZone.restoredToast") : t("dangerZone.archivedToast")),
      onError: (error) =>
        push(
          "error",
          archived ? t("dangerZone.errors.restore") : t("dangerZone.errors.archive"),
          error instanceof ApiError ? error.message : undefined,
        ),
    });
  };

  const { confirm, element: archiveConfirm } = useConfirmDialog();
  const requestArchive = () => {
    confirm({
      title: archived ? t("dangerZone.restoreConfirmTitle") : t("dangerZone.archiveConfirmTitle"),
      description: archived
        ? t("dangerZone.restoreConfirmDescription")
        : t("dangerZone.archiveConfirmDescription"),
      confirmLabel: archived ? t("dangerZone.restoreTeam") : t("dangerZone.archiveTeam"),
      destructive: !archived,
      action: doArchive,
    });
  };

  const deleteTeam = useMutation({
    mutationFn: () => api.deleteTeam(teamId, confirmName.trim(), deletePassword),
    onSuccess: () => {
      push("info", t("dangerZone.deletedToast"), t("dangerZone.deletedBody", { name: teamName }));
      void qc.invalidateQueries({ queryKey: ["teams"] });
      void qc.clear();
      navigate("/", { replace: true });
    },
    onError: (error) =>
      setDeleteError(error instanceof ApiError ? error.message : t("dangerZone.errors.delete")),
  });

  return (
    <section
      aria-label={t("dangerZone.title")}
      className="rounded-sg border border-destructive/40 p-4"
      data-testid="danger-zone"
    >
      {archiveConfirm}
      <h2 className="mb-1 flex items-center gap-1.5 text-sm font-semibold text-destructive">
        <ShieldAlert className="h-4 w-4" /> {t("dangerZone.title")}
      </h2>
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-sm font-medium">{archived ? t("dangerZone.restoreTeam") : t("dangerZone.archiveTeam")}</p>
            <p className="text-2xs text-muted-foreground">
              {archived
                ? t("dangerZone.archivedNote")
                : t("dangerZone.archiveNote")}
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            disabled={archiveTeam.isPending || unarchiveTeam.isPending}
            onClick={requestArchive}
            data-testid="archive-team"
          >
            <Archive /> {archived ? t("dangerZone.restore") : t("dangerZone.archive")}
          </Button>
        </div>
        <Separator />
        <div className="flex items-center justify-between gap-3">
          <div>
            <p className="text-sm font-medium">{t("dangerZone.deleteTeam")}</p>
            <p className="text-2xs text-muted-foreground">
              {t("dangerZone.deleteNote")}
            </p>
          </div>
          <Button
            variant="destructive"
            size="sm"
            disabled={!isOwner}
            title={isOwner ? undefined : t("dangerZone.ownerOnlyTitle")}
            onClick={() => {
              setConfirmName("");
              setDeletePassword("");
              setDeleteError(null);
              setDeleteOpen(true);
            }}
            data-testid="delete-team"
          >
            <Trash2 /> {t("dangerZone.delete")}
          </Button>
        </div>
      </div>

      <DeleteTeamDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        teamName={teamName}
        confirmName={confirmName}
        setConfirmName={setConfirmName}
        deletePassword={deletePassword}
        setDeletePassword={setDeletePassword}
        error={deleteError}
        pending={deleteTeam.isPending}
        onSubmit={() => deleteTeam.mutate()}
      />
    </section>
  );
}

function DeleteTeamDialog({
  open,
  onOpenChange,
  teamName,
  confirmName,
  setConfirmName,
  deletePassword,
  setDeletePassword,
  error,
  pending,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  teamName: string;
  confirmName: string;
  setConfirmName: (v: string) => void;
  deletePassword: string;
  setDeletePassword: (v: string) => void;
  error: string | null;
  pending: boolean;
  onSubmit: () => void;
}) {
  const { t } = useTranslation("settings");
  const ready = confirmName.trim() === teamName && deletePassword.length > 0;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent role="alertdialog">
        <DialogHeader>
          <DialogTitle className="text-destructive">{t("dangerZone.deleteConfirm.title")}</DialogTitle>
          <DialogDescription>
            {t("dangerZone.deleteConfirm.before")}{" "}
            <span className="font-semibold">{teamName}</span>{" "}
            {t("dangerZone.deleteConfirm.after")}
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (ready) onSubmit();
          }}
          noValidate
          data-testid="delete-team-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="delete-confirm-name">{t("dangerZone.teamNameLabel")}</Label>
            <Input
              id="delete-confirm-name"
              value={confirmName}
              onChange={(e) => setConfirmName(e.target.value)}
              placeholder={teamName}
              autoComplete="off"
              data-testid="delete-team-name"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="delete-confirm-password">{t("actions.currentPassword")}</Label>
            <Input
              id="delete-confirm-password"
              type="password"
              value={deletePassword}
              onChange={(e) => setDeletePassword(e.target.value)}
              autoComplete="current-password"
              data-testid="delete-team-password"
            />
          </div>
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)} disabled={pending}>
              {t("actions.cancel")}
            </Button>
            <Button
              variant="destructive"
              type="submit"
              disabled={!ready || pending}
              data-testid="delete-team-submit"
            >
              {pending ? t("dangerZone.deleting") : t("dangerZone.deleteForever")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Labels (unchanged from Phase 3)
// ---------------------------------------------------------------------------

function LabelsSection({ teamId, isAdmin }: { teamId: string; isAdmin: boolean }) {
  const { t } = useTranslation("settings");
  const labels = useLabels(teamId);
  const { push } = useToast();
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [color, setColor] = useState<string>(LABEL_PALETTE[0]);
  const [icon, setIcon] = useState("");
  const [iconSearch, setIconSearch] = useState("");
  // Edit dialog state: the label being edited (null = closed).
  const [editing, setEditing] = useState<LabelData | null>(null);
  const [editName, setEditName] = useState("");
  const [editColor, setEditColor] = useState<string>(LABEL_PALETTE[0]);
  const [editIcon, setEditIcon] = useState("");

  const create = useMutation({
    mutationFn: () => api.createLabel(teamId, name, color, icon || undefined),
    onSuccess: () => {
      push("success", t("labels.created"));
      void qc.invalidateQueries({ queryKey: ["labels", teamId] });
      setName("");
      setIcon("");
    },
    onError: (error) =>
      push("error", t("labels.errors.create"), error instanceof ApiError ? error.message : undefined),
  });

  const update = useMutation({
    mutationFn: (vars: { id: string; name: string; color: string; icon: string | null }) =>
      api.updateLabel(teamId, vars.id, {
        name: vars.name,
        color: vars.color,
        icon: vars.icon,
      }),
    onSuccess: () => {
      push("success", t("labels.updated"));
      void qc.invalidateQueries({ queryKey: ["labels", teamId] });
      setEditing(null);
    },
    onError: (error) =>
      push("error", t("labels.errors.update"), error instanceof Error ? error.message : undefined),
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteLabel(teamId, id),
    onSuccess: () => {
      push("success", t("labels.deleted"));
      void qc.invalidateQueries({ queryKey: ["labels", teamId] });
      setEditing(null);
    },
    onError: (error) =>
      push("error", t("labels.errors.delete"), error instanceof Error ? error.message : undefined),
  });

  const iconEntries = Object.entries(LABEL_ICON_MAP).filter(([key]) =>
    key.includes(iconSearch.trim().toLowerCase()),
  );

  const createForm = isAdmin ? (
    <div
      className="mb-4 flex flex-col gap-3 rounded-xl border bg-card p-4 shadow-sm"
      data-testid="label-create-form"
    >
      {/* Live preview */}
      <div className="flex items-center gap-2">
        <span className="text-2xs text-muted-foreground">{t("labels.preview")}</span>
        {name.trim() ? (
          <LabelChip name={name.trim()} color={color} icon={icon || null} />
        ) : (
          <span className="text-2xs text-muted-foreground">{t("labels.previewHint")}</span>
        )}
      </div>
      <div className="flex items-end gap-2">
        <div className="flex flex-1 flex-col gap-1.5">
          <Label htmlFor="label-name">{t("labels.name")}</Label>
          <Input
            id="label-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("labels.namePlaceholder")}
            aria-label={t("labels.name")}
            dir="auto"
            data-testid="label-name-input"
          />
        </div>
        <Button
          type="button"
          size="sm"
          className="self-start"
          disabled={!name.trim() || create.isPending}
          onClick={() => create.mutate()}
          data-testid="label-add"
        >
          {create.isPending ? t("labels.adding") : t("labels.add")}
        </Button>
      </div>
      <div className="flex flex-col gap-1.5">
        <Label>{t("labels.color")}</Label>
        <div className="flex gap-1.5" role="radiogroup" aria-label={t("labels.colorAria")}>
          {LABEL_PALETTE.map((c) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={color === c}
              aria-label={t("labels.colorSwatchAria", { color: c })}
              onClick={() => setColor(c)}
              className={cn(
                "h-7 w-7 rounded-full border-2 transition-transform duration-100",
                color === c ? "scale-110 border-foreground" : "border-transparent hover:scale-105",
              )}
              style={{ backgroundColor: c }}
              data-testid={`label-color-${c.slice(1)}`}
            />
          ))}
        </div>
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="label-icon-search">{t("labels.iconOptional")}</Label>
        <Input
          id="label-icon-search"
          value={iconSearch}
          onChange={(e) => setIconSearch(e.target.value)}
          placeholder={t("labels.iconSearch")}
          className="w-44"
          data-testid="label-icon-search"
        />
        <div
          className="flex max-h-32 flex-wrap gap-1 overflow-y-auto rounded-md border p-1.5 scrollbar-thin"
          role="listbox"
          aria-label={t("labels.iconAria")}
          data-testid="label-icon-grid"
        >
          <button
            type="button"
            role="option"
            aria-selected={icon === ""}
            onClick={() => setIcon("")}
            className={cn(
              "flex h-7 w-7 items-center justify-center rounded border text-2xs",
              icon === "" ? "border-primary bg-primary/10" : "border-transparent hover:bg-accent",
            )}
            aria-label={t("labels.noIcon")}
          >
            ×
          </button>
          {iconEntries.map(([key, IconComponent]) => (
            <button
              key={key}
              type="button"
              role="option"
              aria-selected={icon === key}
              aria-label={key}
              onClick={() => setIcon(key)}
              className={cn(
                "flex h-7 w-7 items-center justify-center rounded border transition-transform duration-150",
                icon === key
                  ? "scale-110 border-primary bg-primary/10"
                  : "border-transparent hover:bg-accent",
              )}
              data-testid={`label-icon-${key}`}
            >
              <IconComponent className="h-3.5 w-3.5" />
            </button>
          ))}
        </div>
      </div>
    </div>
  ) : null;

  return (
    <section aria-label={t("labels.title")}>
      <h2 className="mb-1 flex items-center gap-1.5 text-sm font-semibold">
        <Tags className="h-4 w-4" /> {t("labels.title")}
      </h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("labels.subtitle")}
      </p>
      {labels.isLoading ? <Skeleton className="h-16 w-full" /> : null}
      {labels.data && labels.data.items.length === 0 && !labels.isLoading ? (
        <p className="text-xs text-muted-foreground" data-testid="labels-empty">
          {t("labels.empty")}
        </p>
      ) : null}
      <div className="mb-4 flex flex-wrap gap-2" data-testid="label-list">
        {labels.data?.items.map((label) =>
          isAdmin ? (
            <button
              key={label.id}
              type="button"
              onClick={() => {
                setEditing(label);
                setEditName(label.name);
                setEditColor(label.color);
                setEditIcon(label.icon ?? "");
              }}
              className="rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              aria-label={t("labels.editAria", { name: label.name })}
              data-testid={`label-edit-${label.id}`}
            >
              <LabelChip name={label.name} color={label.color} icon={label.icon} />
            </button>
          ) : (
            <LabelChip key={label.id} name={label.name} color={label.color} icon={label.icon} />
          ),
        )}
      </div>
      {createForm}

      {/* Edit dialog */}
      <Dialog open={editing !== null} onOpenChange={(v) => !v && setEditing(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("labels.editTitle")}</DialogTitle>
            <DialogDescription>
              {t("labels.editDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-3">
            <div className="flex items-center gap-2">
              <span className="text-2xs text-muted-foreground">{t("labels.preview")}</span>
              <LabelChip name={editName || t("labels.fallbackName")} color={editColor} icon={editIcon || null} />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="edit-label-name">{t("labels.editName")}</Label>
              <Input
                id="edit-label-name"
                value={editName}
                onChange={(e) => setEditName(e.target.value)}
                data-testid="edit-label-name"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>{t("labels.color")}</Label>
              <div className="flex gap-1.5" role="radiogroup" aria-label={t("labels.colorAria")}>
                {LABEL_PALETTE.map((c) => (
                  <button
                    key={c}
                    type="button"
                    role="radio"
                    aria-checked={editColor === c}
                    aria-label={t("labels.colorSwatchAria", { color: c })}
                    onClick={() => setEditColor(c)}
                    className={cn(
                      "h-7 w-7 rounded-full border-2 transition-transform duration-100",
                      editColor === c
                        ? "scale-110 border-foreground"
                        : "border-transparent hover:scale-105",
                    )}
                    style={{ backgroundColor: c }}
                  />
                ))}
              </div>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>{t("labels.icon")}</Label>
              <div className="flex max-h-32 flex-wrap gap-1 overflow-y-auto rounded-md border p-1.5 scrollbar-thin">
                <button
                  type="button"
                  onClick={() => setEditIcon("")}
                  className={cn(
                    "flex h-7 w-7 items-center justify-center rounded border text-2xs",
                    editIcon === ""
                      ? "border-primary bg-primary/10"
                      : "border-transparent hover:bg-accent",
                  )}
                  aria-label={t("labels.noIcon")}
                >
                  ×
                </button>
                {Object.entries(LABEL_ICON_MAP).map(([key, IconComponent]) => (
                  <button
                    key={key}
                    type="button"
                    aria-label={key}
                    onClick={() => setEditIcon(key)}
                    className={cn(
                      "flex h-7 w-7 items-center justify-center rounded border transition-transform duration-150",
                      editIcon === key
                        ? "scale-110 border-primary bg-primary/10"
                        : "border-transparent hover:bg-accent",
                    )}
                  >
                    <IconComponent className="h-3.5 w-3.5" />
                  </button>
                ))}
              </div>
            </div>
            <Button
              variant="destructive"
              size="sm"
              className="self-start"
              disabled={remove.isPending}
              onClick={() => editing && remove.mutate(editing.id)}
              data-testid="label-delete"
            >
              <Trash2 /> {t("labels.delete")}
            </Button>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditing(null)}>
              {t("actions.cancel")}
            </Button>
            <Button
              disabled={!editName.trim() || update.isPending}
              onClick={() =>
                editing &&
                update.mutate({
                  id: editing.id,
                  name: editName.trim(),
                  color: editColor,
                  icon: editIcon || null,
                })
              }
              data-testid="label-save"
            >
              {update.isPending ? t("actions.saving") : t("labels.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

function ProjectTrashSection({ teamId }: { teamId: string }) {
  const { t } = useTranslation("settings");
  const trash = useQuery({
    queryKey: ["project-trash", teamId],
    queryFn: () => api.projectTrash(teamId),
  });
  const { push } = useToast();
  const qc = useQueryClient();
  const [purgeOpen, setPurgeOpen] = useState(false);
  const [purgeTarget, setPurgeTarget] = useState<TrashProject | null>(null);
  const [confirmName, setConfirmName] = useState("");
  const [purgePassword, setPurgePassword] = useState("");
  const [purgeError, setPurgeError] = useState<string | null>(null);

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["project-trash", teamId] });
    void qc.invalidateQueries({ queryKey: ["projects"] });
  };

  const restore = useMutation({
    mutationFn: (id: string) => api.restoreProject(id),
    onSuccess: () => {
      push("success", t("trash.restored"));
      invalidate();
    },
    onError: (error) =>
      push("error", t("trash.errors.restore"), error instanceof ApiError ? error.message : undefined),
  });

  const purge = useMutation({
    mutationFn: () =>
      purgeTarget
        ? api.purgeProject(purgeTarget.id, confirmName.trim(), purgePassword)
        : Promise.reject(new Error("no project selected")),
    onSuccess: () => {
      push("info", t("trash.purged"));
      setPurgeOpen(false);
      setPurgeTarget(null);
      invalidate();
    },
    onError: (error) =>
      setPurgeError(error instanceof ApiError ? error.message : t("trash.errors.purge")),
  });

  const purgeReady = purgeTarget !== null && confirmName.trim() === purgeTarget.name && purgePassword.length > 0;

  return (
    <section aria-label={t("trash.title")} data-testid="project-trash">
      <h2 className="mb-1 flex items-center gap-1.5 text-sm font-semibold">
        <Trash2 className="h-4 w-4" /> {t("trash.title")}
      </h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("trash.subtitle")}
      </p>
      {trash.isLoading ? (
        <Skeleton className="h-16 w-full" />
      ) : (trash.data?.items.length ?? 0) === 0 ? (
        <p className="text-xs text-muted-foreground" data-testid="trash-empty">
          {t("trash.empty")}
        </p>
      ) : (
        <ul className="overflow-hidden rounded-sg border" data-testid="trash-list">
          {trash.data?.items.map((item) => (
            <li key={item.id} className="flex flex-wrap items-center gap-3 border-b px-4 py-2.5 last:border-0">
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{item.name}</p>
                <p className="truncate text-2xs text-muted-foreground">
                  {t("trash.taskCount", {
                    count: item.task_count,
                    deleted: timeAgo(item.deleted_at),
                    ends: item.retention_expires_at ? timeAgo(item.retention_expires_at) : "—",
                  })}
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={restore.isPending}
                onClick={() => restore.mutate(item.id)}
                data-testid={`restore-project-${item.id}`}
              >
                {t("trash.restore")}
              </Button>
              <Button
                variant="destructive"
                size="sm"
                disabled={purge.isPending}
                onClick={() => {
                  setPurgeTarget(item);
                  setConfirmName("");
                  setPurgePassword("");
                  setPurgeError(null);
                  setPurgeOpen(true);
                }}
                data-testid={`purge-project-${item.id}`}
              >
                {t("trash.purge")}
              </Button>
            </li>
          ))}
        </ul>
      )}

      <Dialog open={purgeOpen} onOpenChange={setPurgeOpen}>
        <DialogContent role="alertdialog">
          <DialogHeader>
            <DialogTitle className="text-destructive">{t("trash.purgeConfirm.title")}</DialogTitle>
            <DialogDescription>
              {t("trash.purgeConfirm.before")}{" "}
              <span className="font-semibold">{purgeTarget?.name}</span>{" "}
              {t("trash.purgeConfirm.after")}
            </DialogDescription>
          </DialogHeader>
          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (purgeReady) purge.mutate();
            }}
            noValidate
            data-testid="purge-form"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="purge-confirm-name">{t("trash.projectName")}</Label>
              <Input
                id="purge-confirm-name"
                value={confirmName}
                onChange={(e) => setConfirmName(e.target.value)}
                placeholder={purgeTarget?.name}
                autoComplete="off"
                data-testid="purge-project-name"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="purge-password">{t("actions.currentPassword")}</Label>
              <Input
                id="purge-password"
                type="password"
                value={purgePassword}
                onChange={(e) => setPurgePassword(e.target.value)}
                autoComplete="current-password"
                data-testid="purge-password"
              />
            </div>
            {purgeError ? <p className="text-2xs text-destructive" role="alert">{purgeError}</p> : null}
            <DialogFooter>
              <Button variant="outline" onClick={() => setPurgeOpen(false)} disabled={purge.isPending}>
                {t("actions.cancel")}
              </Button>
              <Button
                variant="destructive"
                type="submit"
                disabled={!purgeReady || purge.isPending}
                data-testid="purge-submit"
              >
                {purge.isPending ? t("trash.purging") : t("trash.deleteForever")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </section>
  );
}
