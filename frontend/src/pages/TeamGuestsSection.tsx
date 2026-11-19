import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Check, Search, ShieldCheck, Trash2, X } from "lucide-react";
import {
  api,
  ApiError,
  type GuestInvitationRequest,
  type GuestPermissionKey,
  type TeamGuest2,
} from "@/api/client";
import {
  GuestPermissionDrawer,
  PermissionBadge,
  PERM_META,
  DOMAIN_LABELS,
  type PermDomain,
} from "./GuestPermissionDrawer";
import { adminApi, type EligibleMember } from "@/api/admin-client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/badge";
import { Avatar } from "@/components/ui/avatar";
import { useToast } from "@/components/ui/toast";
import { useConfirmDialog } from "@/components/ui/confirm-dialog";
import { timeAgo } from "@/lib/utils";

/**
 * Team guests (Phase 13 Bug 7, ADR-0018; granular since Phase 14 WS2,
 * ADR-0021): non-members with an explicitly granted permission set. Deny
 * by default; revocation is immediate. Includes the two-step invitation
 * queue: guests with `invite_guest` file requests, Owner/Admin approve or
 * reject. Coexists with the Phase 3 project-viewer guest list.
 */

export function TeamGuestsSection({
  teamId,
  projects,
  canManage,
}: {
  teamId: string;
  /** The team's projects (for per-project permission scopes). */
  projects: { id: string; name: string }[];
  canManage: boolean;
}) {
  const { t } = useTranslation("settings");
  const qc = useQueryClient();
  const { push } = useToast();
  const { confirm, element: confirmElement } = useConfirmDialog();
  const [inviteOpen, setInviteOpen] = useState(false);
  const [drawerGuest, setDrawerGuest] = useState<TeamGuest2 | null>(null);

  const guests = useQuery({
    queryKey: ["team-guests2", teamId],
    queryFn: () => api.listTeamGuests2(teamId),
  });

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["team-guests2", teamId] });
  };

  const revoke = useMutation({
    mutationFn: (guestId: string) => api.revokeTeamGuest2(teamId, guestId),
    onSuccess: () => {
      push("success", t("guests.accessRevoked"));
      invalidate();
    },
    onError: (error) =>
      push("error", t("guests.errors.revoke"), error instanceof Error ? error.message : undefined),
  });


  const revokePermission = useMutation({
    mutationFn: (vars: { guestId: string; permissionId: string }) =>
      api.revokeGuestPermission(teamId, vars.guestId, vars.permissionId),
    onSuccess: () => {
      push("success", t("teamGuests.permissionRemoved"));
      invalidate();
    },
    onError: (error) =>
      push("error", t("teamGuests.errors.removePermission"), error instanceof Error ? error.message : undefined),
  });

  const invitations = useQuery({
    queryKey: ["guest-invite-requests", teamId],
    queryFn: () => api.listGuestInviteRequests(teamId),
    enabled: canManage,
  });

  const decide = useMutation({
    mutationFn: (vars: { requestId: string; decision: "approve" | "reject" }) =>
      api.decideGuestInviteRequest(teamId, vars.requestId, vars.decision),
    onSuccess: (row) => {
      push(
        "success",
        row.status === "approved" ? t("teamGuests.queue.invitationApproved") : t("teamGuests.queue.invitationRejected"),
        row.invitee_email,
      );
      void qc.invalidateQueries({ queryKey: ["guest-invite-requests", teamId] });
      invalidate();
    },
    onError: (error) =>
      push(
        "error",
        t("teamGuests.errors.decide"),
        error instanceof Error ? error.message : undefined,
      ),
  });

  return (
    <section aria-label={t("teamGuests.title")}>
      {confirmElement}
      <div className="mb-1 flex items-center justify-between">
        <h2 className="text-sm font-semibold">{t("teamGuests.title")}</h2>
        {canManage ? (
          <Button size="sm" variant="outline" onClick={() => setInviteOpen(true)} data-testid="invite-guest">
            <ShieldCheck /> {t("teamGuests.addGuest")}
          </Button>
        ) : null}
      </div>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("teamGuests.subtitle")}
      </p>
      {guests.isLoading ? (
        <Skeleton className="h-16 w-full" />
      ) : (guests.data?.items.length ?? 0) === 0 ? (
        <p className="text-xs text-muted-foreground" data-testid="team-guests-empty">
          {t("teamGuests.empty")}
        </p>
      ) : (
        <ul className="flex flex-col gap-2" data-testid="team-guest-list">
          {guests.data!.items.map((guest) => (
            <li key={guest.guest_id} className="rounded-sg border p-3">
              <div className="flex items-center gap-3">
                <Avatar name={guest.user.display_name} size="md" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">
                    {guest.user.display_name}
                    <span className="ms-1.5 truncate text-2xs text-muted-foreground">
                      {guest.user.email}
                    </span>
                  </p>
                  <p className="text-2xs text-muted-foreground">
                    {t("teamGuests.grantedAgo", { time: timeAgo(guest.created_at) })}
                  </p>
                </div>
                {canManage ? (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={revoke.isPending}
                    onClick={() =>
                      confirm({
                        title: t("guests.revokeConfirm.title"),
                        description: t("teamGuests.revokeDescription", { name: guest.user.display_name }),
                        confirmLabel: t("guests.revokeConfirm.confirm"),
                        action: () => revoke.mutate(guest.guest_id),
                      })
                    }
                    data-testid={`revoke-team-guest-${guest.guest_id}`}
                  >
                    <Trash2 /> {t("actions.revoke")}
                  </Button>
                ) : null}
              </div>
              {/* Permission matrix: granted rows grouped by domain + the
                  grant controls (ADR-0021). */}
              <div className="mt-2.5 flex flex-col gap-2">
                <div
                  className="flex flex-col gap-1.5"
                  data-testid={`guest-badges-${guest.guest_id}`}
                >
                {guest.permissions.length === 0 ? (
                  <span className="text-2xs text-muted-foreground">{t("teamGuests.noPermissions")}</span>
                ) : (
                  <>
                    {(["team", "projects", "tasks", "admin"] as PermDomain[]).map((domain) => {
                      const rows = guest.permissions.filter(
                        (p) =>
                          (PERM_META[p.permission as GuestPermissionKey] ?? PERM_META.view_team)
                            .domain === domain,
                      );
                      if (rows.length === 0) return null;
                      return (
                        <div key={domain} className="flex flex-wrap items-center gap-1.5">
                          <span className="w-20 shrink-0 text-2xs uppercase tracking-wide text-muted-foreground">
                            {t(DOMAIN_LABELS[domain])}
                          </span>
                          {rows.map((p) => (
                            <PermissionBadge
                              key={p.id}
                              permission={p}
                              onRemove={
                                canManage
                                  ? () =>
                                      revokePermission.mutate({
                                        guestId: guest.guest_id,
                                        permissionId: p.id,
                                      })
                                  : undefined
                              }
                            />
                          ))}
                        </div>
                      );
                    })}
                  </>
                )}
                </div>
                {canManage ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="self-start"
                    onClick={() => setDrawerGuest(guest)}
                    data-testid={`edit-permissions-${guest.guest_id}`}
                  >
                    <ShieldCheck /> {t("teamGuests.editPermissions")}
                  </Button>
                ) : null}
              </div>
            </li>
          ))}
        </ul>
      )}
      {inviteOpen ? (
        <InviteGuestDialog teamId={teamId} onOpenChange={setInviteOpen} />
      ) : null}
      {canManage && (invitations.data?.items.length ?? 0) > 0 ? (
        <InvitationQueue
          requests={invitations.data!.items}
          onDecide={(requestId, decision) => decide.mutate({ requestId, decision })}
          pending={decide.isPending}
        />
      ) : null}
      {drawerGuest ? (
        <GuestPermissionDrawer
          teamId={teamId}
          guest={drawerGuest}
          projects={projects}
          onClose={() => setDrawerGuest(null)}
        />
      ) : null}
    </section>
  );
}


function InviteGuestDialog({
  teamId,
  onOpenChange,
}: {
  teamId: string;
  onOpenChange: (v: boolean) => void;
}) {
  const { t } = useTranslation("settings");
  const { push } = useToast();
  const qc = useQueryClient();
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query]);

  const eligible = useQuery({
    queryKey: ["eligible-members", teamId, debounced],
    queryFn: () => adminApi.eligibleMembers(teamId, { q: debounced || undefined, page_size: 20 }),
    placeholderData: (prev) => prev,
  });

  const invite = useMutation({
    mutationFn: (email: string) => api.inviteTeamGuest2(teamId, email),
    onSuccess: (_guest, email) => {
      push("success", t("teamGuests.addedToast"), email);
      void qc.invalidateQueries({ queryKey: ["team-guests2", teamId] });
      void qc.invalidateQueries({ queryKey: ["eligible-members", teamId] });
      onOpenChange(false);
    },
    onError: (err) =>
      setError(err instanceof ApiError ? err.message : t("teamGuests.errors.add")),
  });

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div
        role="dialog"
        aria-modal
        aria-label={t("teamGuests.addGuest")}
        className="w-full max-w-md rounded-sg border bg-popover p-4 shadow-lg"
      >
        <h2 className="text-sm font-semibold">{t("teamGuests.addGuest")}</h2>
        <p className="mb-3 text-xs text-muted-foreground">
          {t("teamGuests.addDescription")}
        </p>
        <div className="relative mb-2">
          <Search className="absolute start-2.5 top-2.5 h-4 w-4 text-muted-foreground" aria-hidden />
          <Input
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setDebounced(e.target.value.trim());
            }}
            placeholder={t("addMember.searchPlaceholder")}
            aria-label={t("teamGuests.findUserAria")}
            className="ps-8"
            autoFocus
            dir="auto"
            data-testid="guest-search-input"
          />
        </div>
        <ul
          className="max-h-56 overflow-y-auto rounded-md border p-1 scrollbar-thin"
          role="listbox"
          aria-label={t("addMember.eligibleAria")}
          data-testid="guest-eligible-list"
        >
          {eligible.isLoading ? (
            <li className="flex flex-col gap-1.5 p-1.5">
              <Skeleton className="h-7 w-full" />
              <Skeleton className="h-7 w-4/5" />
            </li>
          ) : (eligible.data?.items.length ?? 0) === 0 ? (
            <li className="p-3 text-center text-xs text-muted-foreground" data-testid="guest-eligible-empty">
              {t("addMember.noMatch")}
            </li>
          ) : (
            eligible.data!.items.map((u: EligibleMember) => (
              <li key={u.id} role="none">
                <button
                  type="button"
                  role="option"
                  aria-selected={false}
                  onClick={() => invite.mutate(u.email)}
                  className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-start text-sm hover:bg-accent"
                >
                  <span className="min-w-0 flex-1 truncate">
                    <span className="font-medium">{u.display_name}</span>{" "}
                    <span className="text-2xs text-muted-foreground">{u.email}</span>
                  </span>
                  {u.username ? (
                    <span className="shrink-0 text-2xs text-muted-foreground">@{u.username}</span>
                  ) : null}
                </button>
              </li>
            ))
          )}
        </ul>
        {error ? <p className="mt-2 text-2xs text-destructive" role="alert">{error}</p> : null}
        <div className="mt-3 flex justify-end">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("actions.cancel")}
          </Button>
        </div>
      </div>
    </div>
  );
}

/**
 * Two-step invitation queue (Phase 14 WS2, ADR-0021): pending requests
 * filed by `invite_guest` holders, then decided history. Owner/Admin only.
 */
function InvitationQueue({
  requests,
  onDecide,
  pending,
}: {
  requests: GuestInvitationRequest[];
  onDecide: (requestId: string, decision: "approve" | "reject") => void;
  pending: boolean;
}) {
  const { t } = useTranslation("settings");
  const pendingItems = requests.filter((r) => r.status === "pending");
  const decided = requests.filter((r) => r.status !== "pending");
  if (requests.length === 0) return null;
  return (
    <div
      className="mt-4 rounded-sg border p-3"
      aria-label={t("teamGuests.queue.aria")}
      data-testid="invite-requests-list"
    >
      <h3 className="mb-1.5 text-xs font-semibold">{t("teamGuests.queue.title")}</h3>
      <p className="mb-2 text-2xs text-muted-foreground">
        {t("teamGuests.queue.descriptionBefore")} <code>invite_guest</code>{" "}
        {t("teamGuests.queue.descriptionAfter")}
      </p>
      {pendingItems.length === 0 && decided.length === 0 ? null : (
        <ul className="flex flex-col gap-1.5">
          {pendingItems.map((r) => (
            <li
              key={r.id}
              className="flex items-center justify-between gap-2 rounded-md border border-dashed px-2.5 py-2"
              data-testid={`invite-request-${r.id}`}
            >
              <div className="min-w-0">
                <p className="truncate text-xs font-medium">{r.invitee_email}</p>
                <p className="text-2xs text-muted-foreground">
                  {t("teamGuests.queue.requestedBy", {
                    name: r.requested_by?.display_name ?? t("teamGuests.queue.unknownUser"),
                    time: timeAgo(r.created_at),
                  })}
                </p>
              </div>
              <div className="flex shrink-0 gap-1.5">
                <Button
                  size="sm"
                  disabled={pending}
                  onClick={() => onDecide(r.id, "approve")}
                  data-testid={`approve-invite-request-${r.id}`}
                >
                  <Check /> {t("teamGuests.queue.approve")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={pending}
                  onClick={() => onDecide(r.id, "reject")}
                  data-testid={`reject-invite-request-${r.id}`}
                >
                  <X /> {t("teamGuests.queue.reject")}
                </Button>
              </div>
            </li>
          ))}
          {decided.map((r) => (
            <li
              key={r.id}
              className="flex items-center justify-between gap-2 rounded-md px-2.5 py-1.5"
            >
              <div className="min-w-0">
                <p className="truncate text-2xs text-muted-foreground">{r.invitee_email}</p>
              </div>
              <span
                className={
                  r.status === "approved"
                    ? "shrink-0 text-2xs font-medium text-emerald-600 dark:text-emerald-400"
                    : "shrink-0 text-2xs font-medium text-muted-foreground"
                }
              >
                {r.status === "approved" ? t("teamGuests.queue.approved") : t("teamGuests.queue.rejected")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
