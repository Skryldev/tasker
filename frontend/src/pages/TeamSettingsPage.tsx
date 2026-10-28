import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Plus, Tags, Trash2, UserPlus, Users } from "lucide-react";
import { api, ApiError, type Member } from "@/api/client";
import { useLabels, useMembers, useTeam } from "@/api/hooks";
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
import { useToast } from "@/components/ui/toast";
import { useWorkspace, useIsAdmin } from "@/lib/workspace";
import { roleLabel, timeAgo } from "@/lib/utils";

const ROLES = ["owner", "admin", "manager", "member"] as const;

export function TeamSettingsPage() {
  const { teamId } = useWorkspace();
  const team = useTeam(teamId);
  const isAdmin = useIsAdmin();

  if (!teamId || team.isError) {
    return (
      <EmptyState
        icon={<Users className="h-8 w-8" />}
        title="Team unavailable"
        description="Select a valid team from the switcher."
      />
    );
  }

  return (
    <div className="mx-auto max-w-3xl p-6">
      <header className="mb-6">
        <h1 className="text-lg font-semibold">{team.data?.name ?? "Team"} settings</h1>
        <p className="text-xs text-muted-foreground">
          {team.data?.description || "Manage members, roles, and labels."}
        </p>
      </header>

      {isAdmin ? (
        <>
          <section aria-label="Team details" className="mb-8">
            <h2 className="mb-3 text-sm font-semibold">Team details</h2>
            <TeamDetailsForm teamId={teamId} initialName={team.data?.name ?? ""} initialDescription={team.data?.description ?? ""} />
          </section>
          <Separator className="mb-8" />
        </>
      ) : null}

      <MembersSection teamId={teamId} isAdmin={isAdmin} />
      <Separator className="my-8" />
      <LabelsSection teamId={teamId} isAdmin={isAdmin} />
    </div>
  );
}

function TeamDetailsForm({
  teamId,
  initialName,
  initialDescription,
}: {
  teamId: string;
  initialName: string;
  initialDescription: string;
}) {
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const { push } = useToast();
  const qc = useQueryClient();
  const save = useMutation({
    mutationFn: () => api.updateTeam(teamId, { name, description }),
    onSuccess: () => {
      push("success", "Team updated");
      void qc.invalidateQueries({ queryKey: ["teams"] });
      void qc.invalidateQueries({ queryKey: ["team", teamId] });
    },
    onError: (error) => push("error", "Could not update team", error.message),
  });

  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate();
      }}
    >
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="team-name">Name</Label>
        <Input id="team-name" value={name} onChange={(e) => setName(e.target.value)} className="max-w-sm" />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="team-description">Description</Label>
        <Input
          id="team-description"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          className="max-w-sm"
        />
      </div>
      <Button type="submit" size="sm" className="self-start" disabled={save.isPending || !name.trim()}>
        {save.isPending ? "Saving…" : "Save"}
      </Button>
    </form>
  );
}

function MembersSection({ teamId, isAdmin }: { teamId: string; isAdmin: boolean }) {
  const members = useMembers(teamId);
  const { push } = useToast();
  const qc = useQueryClient();
  const [addOpen, setAddOpen] = useState(false);

  const invalidate = () => void qc.invalidateQueries({ queryKey: ["members", teamId] });

  const changeRole = useMutation({
    mutationFn: ({ userId, role }: { userId: string; role: string }) =>
      api.changeMemberRole(teamId, userId, role),
    onSuccess: () => {
      push("success", "Role updated");
      invalidate();
    },
    onError: (error) =>
      push("error", "Could not change role", error instanceof ApiError ? error.message : error.message),
  });

  const removeMember = useMutation({
    mutationFn: (userId: string) => api.removeMember(teamId, userId),
    onSuccess: () => {
      push("info", "Member removed");
      invalidate();
    },
    onError: (error) =>
      push("error", "Could not remove member", error instanceof ApiError ? error.message : error.message),
  });

  const confirmRemove = (member: Member) => {
    if (window.confirm(`Remove ${member.user.display_name} from the team?`)) {
      removeMember.mutate(member.user.id);
    }
  };

  return (
    <section aria-label="Team members">
      <div className="mb-3 flex items-center justify-between">
        <h2 className="text-sm font-semibold">Members ({members.data?.items.length ?? 0})</h2>
        {isAdmin ? (
          <Button size="sm" variant="outline" onClick={() => setAddOpen(true)} data-testid="add-member">
            <UserPlus /> Add member
          </Button>
        ) : null}
      </div>
      {members.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : (
        <ul className="overflow-hidden rounded-lg border" data-testid="member-list">
          {members.data?.items.map((member) => (
            <li key={member.user.id} className="flex items-center gap-3 border-b px-4 py-2.5 last:border-0">
              <Avatar name={member.user.display_name} size="md" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">
                  {member.user.display_name}
                  {member.role === "owner" ? (
                    <Badge className="ml-2" variant="muted">
                      {roleLabel(member.role)}
                    </Badge>
                  ) : null}
                </p>
                <p className="truncate text-2xs text-muted-foreground">
                  {member.user.email} · joined {timeAgo(member.created_at)}
                </p>
              </div>
              {isAdmin ? (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="icon-sm" aria-label={`Manage ${member.user.display_name}`}>
                      <MoreHorizontal />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    {ROLES.filter((r) => r !== member.role).map((role) => (
                      <DropdownMenuItem
                        key={role}
                        onSelect={() => changeRole.mutate({ userId: member.user.id, role })}
                      >
                        Make {roleLabel(role)}
                      </DropdownMenuItem>
                    ))}
                    <DropdownMenuItem destructive onSelect={() => confirmRemove(member)}>
                      <Trash2 /> Remove from team
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              ) : (
                <Badge variant="muted">{roleLabel(member.role)}</Badge>
              )}
            </li>
          ))}
        </ul>
      )}
      {addOpen ? <AddMemberDialog open={addOpen} onOpenChange={setAddOpen} teamId={teamId} /> : null}
    </section>
  );
}

function AddMemberDialog({
  open,
  onOpenChange,
  teamId,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  teamId: string;
}) {
  const { push } = useToast();
  const qc = useQueryClient();
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [role, setRole] = useState("member");
  const [tempPassword, setTempPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () =>
      api.addMember(teamId, {
        email,
        display_name: displayName || undefined,
        role,
        temp_password: tempPassword || undefined,
      }),
    onSuccess: () => {
      push("success", "Member added", email);
      void qc.invalidateQueries({ queryKey: ["members", teamId] });
      setEmail("");
      setDisplayName("");
      setTempPassword("");
      setError(null);
      onOpenChange(false);
    },
    onError: (err) => setError(err instanceof ApiError ? err.message : "Could not add the member."),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add member</DialogTitle>
          <DialogDescription>
            If the person has no account yet, a temporary password creates one — hand it to them
            securely; they should change it after signing in.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!email.trim()) {
              setError("Email is required.");
              return;
            }
            if (!tempPassword.trim() && tempPassword !== "") {
              setError("Temporary password must be at least 10 characters.");
              return;
            }
            setError(null);
            add.mutate();
          }}
          noValidate
          data-testid="add-member-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="member-email">Email</Label>
            <Input
              id="member-email"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="teammate@company.com"
              autoFocus
              data-testid="member-email-input"
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="member-name">Display name</Label>
              <Input
                id="member-name"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder="Optional"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="member-role">Role</Label>
              <Select value={role} onValueChange={setRole}>
                <SelectTrigger id="member-role">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {ROLES.map((r) => (
                    <SelectItem key={r} value={r}>
                      {roleLabel(r)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="member-password">Temporary password (new accounts only)</Label>
            <Input
              id="member-password"
              type="text"
              value={tempPassword}
              onChange={(e) => setTempPassword(e.target.value)}
              placeholder="Min. 10 characters — leave empty for existing users"
              autoComplete="off"
            />
          </div>
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={add.isPending} data-testid="member-submit">
              {add.isPending ? "Adding…" : "Add member"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function LabelsSection({ teamId, isAdmin }: { teamId: string; isAdmin: boolean }) {
  const labels = useLabels(teamId);
  const { push } = useToast();
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [color, setColor] = useState("#6366f1");

  const create = useMutation({
    mutationFn: () => api.createLabel(teamId, name, color),
    onSuccess: () => {
      push("success", "Label created");
      void qc.invalidateQueries({ queryKey: ["labels", teamId] });
      setName("");
    },
    onError: (error) =>
      push("error", "Could not create label", error instanceof ApiError ? error.message : error.message),
  });

  return (
    <section aria-label="Labels">
      <h2 className="mb-1 flex items-center gap-1.5 text-sm font-semibold">
        <Tags className="h-4 w-4" /> Labels
      </h2>
      <p className="mb-3 text-xs text-muted-foreground">
        Categorize tasks. Members with the Manager role or above can create them.
      </p>
      {labels.isLoading ? <Skeleton className="h-16 w-full" /> : null}
      {labels.data && labels.data.items.length === 0 && !labels.isLoading ? (
        <p className="text-xs text-muted-foreground">No labels yet.</p>
      ) : null}
      <div className="mb-4 flex flex-wrap gap-2" data-testid="label-list">
        {labels.data?.items.map((label) => (
          <Badge key={label.id} variant="outline" className="gap-1.5 py-1">
            <span className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: label.color }} />
            {label.name}
          </Badge>
        ))}
      </div>
      {isAdmin ? (
        <form
          className="flex items-end gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (name.trim()) create.mutate();
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="label-name" className="sr-only">
              Label name
            </Label>
            <Input
              id="label-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="New label name"
              className="w-44"
            />
          </div>
          <input
            type="color"
            value={color}
            onChange={(e) => setColor(e.target.value)}
            aria-label="Label color"
            className="h-8 w-10 cursor-pointer rounded border border-input bg-background"
          />
          <Button type="submit" size="sm" variant="outline" disabled={!name.trim() || create.isPending}>
            <Plus /> Add
          </Button>
        </form>
      ) : null}
    </section>
  );
}
