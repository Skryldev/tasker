import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Search, ShieldOff, ShieldCheck, Trash2, UserPlus } from "lucide-react";
import { ApiError, type AdminUser } from "@/api/client";
import { adminApi } from "@/api/admin-client";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { EmptyState } from "@/components/app/EmptyState";
import { useToast } from "@/components/ui/toast";
import { Timestamp } from "@/components/app/Timestamp";

/**
 * Admin → Users (Phase 12.5 Bug 2): instance-wide account administration,
 * Owner-only. Accounts are created here without team membership; team
 * assignment happens in Team Settings → Members.
 */

const PASSWORD_GENERATOR_ALPHABET = {
  lower: "abcdefghijkmnopqrstuvwxyz",
  upper: "ABCDEFGHJKLMNPQRSTUVWXYZ",
  digit: "23456789",
  special: "-_!",
};

/** Generate a policy-passing temporary password (no dependency added). */
function generateTemporaryPassword(): string {
  const rand = (n: number) => Math.floor(Math.random() * n);
  const parts = [
    Array.from({ length: 4 }, () => PASSWORD_GENERATOR_ALPHABET.lower[rand(24)]),
    Array.from({ length: 4 }, () => PASSWORD_GENERATOR_ALPHABET.upper[rand(23)]),
    Array.from({ length: 4 }, () => PASSWORD_GENERATOR_ALPHABET.digit[rand(8)]),
    Array.from({ length: 3 }, () => PASSWORD_GENERATOR_ALPHABET.special[rand(3)]),
  ].flat();
  // Shuffle so the classes are not positional, then join.
  for (let i = parts.length - 1; i > 0; i -= 1) {
    const j = rand(i + 1);
    [parts[i], parts[j]] = [parts[j], parts[i]];
  }
  return parts.join("");
}

export default function AdminUsersPage() {
  const { push } = useToast();
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<AdminUser | null>(null);
  const [deleteConfirmEmail, setDeleteConfirmEmail] = useState("");
  const [deletePassword, setDeletePassword] = useState("");

  useEffect(() => {
    const t = setTimeout(() => setDebounced(search.trim()), 250);
    return () => clearTimeout(t);
  }, [search]);

  const users = useQuery({
    queryKey: ["admin-users", debounced],
    queryFn: () => adminApi.adminUsers({ q: debounced || undefined, page_size: 50 }),
    placeholderData: (prev) => prev,
  });

  const setActive = useMutation({
    mutationFn: ({ id, active }: { id: string; active: boolean }) =>
      adminApi.updateAdminUser(id, { active }),
    onSuccess: (_user, vars) => {
      push("success", vars.active ? "Account activated" : "Account deactivated");
      void qc.invalidateQueries({ queryKey: ["admin-users"] });
    },
    onError: (error) =>
      push("error", "Could not update account", error instanceof Error ? error.message : undefined),
  });

  const hardDelete = useMutation({
    mutationFn: () =>
      adminApi.deleteAdminUser(deleteTarget!.id, deleteConfirmEmail.trim(), deletePassword),
    onSuccess: () => {
      push("success", "Account deleted");
      setDeleteTarget(null);
      setDeleteConfirmEmail("");
      setDeletePassword("");
      void qc.invalidateQueries({ queryKey: ["admin-users"] });
    },
    onError: (error) => {
      const message =
        error instanceof ApiError
          ? error.problem.errors?.[0]?.message ?? error.message
          : "Could not delete the account.";
      push("error", "Delete refused", message);
    },
  });

  const items = users.data?.items ?? [];
  const emptyFiltered = debounced.length > 0 && items.length === 0;

  return (
    <div className="mx-auto w-full max-w-4xl p-4" data-testid="admin-users-page">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">Users</h1>
          <p className="text-2xs text-muted-foreground">
            Instance-wide accounts — created here by an Owner, assigned to teams separately.
          </p>
        </div>
        <Button size="sm" onClick={() => setCreateOpen(true)} data-testid="admin-create-user">
          <UserPlus /> Create user
        </Button>
      </div>

      <div className="relative mb-3">
        <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" aria-hidden />
        <Input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search by email, name or username…"
          aria-label="Search users"
          className="pl-8"
          data-testid="admin-users-search"
        />
      </div>

      {users.isLoading ? (
        <div className="flex flex-col gap-2" aria-busy="true">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
        </div>
      ) : users.isError ? (
        <EmptyState
          icon={<UserPlus className="h-6 w-6" />}
          title="Could not load users"
          description="You may not be an instance Owner."
          action={
            <Button variant="outline" size="sm" onClick={() => void users.refetch()}>
              Retry
            </Button>
          }
        />
      ) : emptyFiltered ? (
        <EmptyState
          icon={<Search className="h-6 w-6" />}
          title="No users match"
          description="Try a different search."
        />
      ) : (
        <div className="overflow-x-auto rounded-lg border" data-testid="admin-users-table">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-b bg-muted/50 text-left">
                {["User", "Email", "Username", "Teams", "Status", "Created", ""].map((h) => (
                  <th
                    key={h}
                    className="px-3 py-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground"
                  >
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {items.map((u) => (
                <tr key={u.id} className="h-11 border-b transition-colors last:border-0 hover:bg-accent/40">
                  <td className="px-3 font-medium">{u.display_name}</td>
                  <td className="max-w-[16rem] truncate px-3 text-muted-foreground">{u.email}</td>
                  <td className="px-3 text-muted-foreground">{u.username ? `@${u.username}` : "—"}</td>
                  <td className="px-3">
                    <span className="flex flex-wrap gap-1">
                      {u.teams.length === 0 ? (
                        <span className="text-2xs text-muted-foreground">No teams</span>
                      ) : (
                        u.teams.map((t) => (
                          <span
                            key={t.id}
                            className="rounded-full border bg-muted/50 px-1.5 py-px text-2xs text-muted-foreground"
                          >
                            {t.name}
                          </span>
                        ))
                      )}
                    </span>
                  </td>
                  <td className="px-3">
                    <span
                      className={
                        u.active
                          ? "inline-flex rounded-full border border-success/40 bg-success/10 px-1.5 py-0.5 text-2xs text-success"
                          : "inline-flex rounded-full border border-destructive/40 bg-destructive/10 px-1.5 py-0.5 text-2xs text-destructive"
                      }
                      data-testid={`user-status-${u.id}`}
                    >
                      {u.active ? "Active" : "Deactivated"}
                    </span>
                  </td>
                  <td className="px-3 text-2xs text-muted-foreground"><Timestamp value={u.created_at} /></td>
                  <td className="px-3">
                    <span className="flex items-center justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={u.active ? `Deactivate ${u.display_name}` : `Activate ${u.display_name}`}
                        onClick={() => setActive.mutate({ id: u.id, active: !u.active })}
                        data-testid={`user-toggle-${u.id}`}
                      >
                        {u.active ? <ShieldOff /> : <ShieldCheck />}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`Delete ${u.display_name}`}
                        onClick={() => setDeleteTarget(u)}
                        data-testid={`user-delete-${u.id}`}
                      >
                        <Trash2 className="text-muted-foreground hover:text-destructive" />
                      </Button>
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <CreateUserDialog open={createOpen} onOpenChange={setCreateOpen} />

      <ConfirmDialog
        open={deleteTarget !== null}
        onOpenChange={(v) => {
          if (!v) setDeleteTarget(null);
        }}
        title={`Delete ${deleteTarget?.display_name ?? "this account"}?`}
        description="Soft-deactivate happens automatically. A hard delete requires typing the account's email and your password — it is refused while the account still has teams, tasks or comments."
        confirmLabel={deleteConfirmEmail && deletePassword ? "Hard delete" : "Deactivate"}
        pending={hardDelete.isPending}
        onConfirm={() => {
          if (deleteConfirmEmail.trim() && deletePassword) {
            hardDelete.mutate();
            return;
          }
          if (deleteTarget) {
            setActive.mutate(
              { id: deleteTarget.id, active: false },
              {
                onSuccess: () => {
                  push("info", "Account deactivated");
                  setDeleteTarget(null);
                },
              },
            );
          }
        }}
      >
        {deleteTarget ? (
          <div className="flex flex-col gap-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="delete-confirm-email">Type the account email to hard-delete</Label>
              <Input
                id="delete-confirm-email"
                value={deleteConfirmEmail}
                onChange={(e) => setDeleteConfirmEmail(e.target.value)}
                placeholder={deleteTarget.email}
                autoComplete="off"
                data-testid="delete-confirm-email"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="delete-confirm-password">Your password</Label>
              <Input
                id="delete-confirm-password"
                type="password"
                value={deletePassword}
                onChange={(e) => setDeletePassword(e.target.value)}
                autoComplete="current-password"
                data-testid="delete-confirm-password"
              />
            </div>
            <p className="text-2xs text-muted-foreground">
              Leave both fields empty to just deactivate the account.
            </p>
          </div>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}

function CreateUserDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const { push } = useToast();
  const qc = useQueryClient();
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [username, setUsername] = useState("");
  const [tempPassword, setTempPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setEmail("");
      setDisplayName("");
      setUsername("");
      setTempPassword(generateTemporaryPassword());
      setError(null);
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      adminApi.createAdminUser({
        email: email.trim(),
        display_name: displayName.trim(),
        username: username.trim() || undefined,
        temporary_password: tempPassword,
      }),
    onSuccess: (user) => {
      push("success", "Account created", user.email);
      void qc.invalidateQueries({ queryKey: ["admin-users"] });
      onOpenChange(false);
    },
    onError: (err) => {
      if (err instanceof ApiError && err.problem.errors) {
        setError(err.problem.errors.map((e) => `${e.field}: ${e.message}`).join(" · "));
      } else {
        setError(err instanceof Error ? err.message : "Could not create the account.");
      }
    },
  });

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!email.trim() || !displayName.trim()) {
      setError("Email and display name are required.");
      return;
    }
    setError(null);
    create.mutate();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create instance user</DialogTitle>
          <DialogDescription>
            The account exists instance-wide — assign it to teams from each team's settings.
          </DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-3" onSubmit={submit} noValidate data-testid="create-user-form">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-user-email">Email</Label>
            <Input
              id="new-user-email"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="teammate@company.com"
              autoFocus
              data-testid="new-user-email"
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-user-name">Display name</Label>
              <Input
                id="new-user-name"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                data-testid="new-user-name"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="new-user-username">Username (optional)</Label>
              <Input
                id="new-user-username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                placeholder="3-32 chars: a-z 0-9 _"
                data-testid="new-user-username"
              />
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-user-password">Temporary password</Label>
            <div className="flex gap-2">
              <Input
                id="new-user-password"
                type="text"
                value={tempPassword}
                onChange={(e) => setTempPassword(e.target.value)}
                autoComplete="off"
                data-testid="new-user-password"
              />
              <Button
                type="button"
                variant="outline"
                onClick={() => setTempPassword(generateTemporaryPassword())}
                data-testid="generate-password"
              >
                Generate
              </Button>
            </div>
            <p className="text-2xs text-muted-foreground">
              ≥ 12 characters with upper, lower, digit and special — enforced server-side.
            </p>
          </div>
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending} data-testid="create-user-submit">
              {create.isPending ? "Creating…" : "Create user"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
