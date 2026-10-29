import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2, UserPlus, Users } from "lucide-react";
import { api, ApiError } from "@/api/client";
import { Avatar } from "@/components/ui/avatar";
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
import { Input, Label } from "@/components/ui/input";
import { EmptyState } from "@/components/app/EmptyState";
import { useToast } from "@/components/ui/toast";
import { timeAgo } from "@/lib/utils";

/** Guest roster management for one project (Manager+). Guests get read-only
 *  access to this project only. */
export function ProjectGuestsPage() {
  const { projectId } = useParams<{ projectId: string }>();
  const qc = useQueryClient();
  const { push } = useToast();
  const [addOpen, setAddOpen] = useState(false);
  const [email, setEmail] = useState("");
  const [error, setError] = useState<string | null>(null);

  const project = useQuery({
    queryKey: ["project", projectId],
    queryFn: () => api.getProject(projectId!),
    enabled: Boolean(projectId),
  });
  const guests = useQuery({
    queryKey: ["guests", projectId],
    queryFn: () => api.listGuests(projectId!),
    enabled: Boolean(projectId),
  });

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["guests", projectId] });
  };

  const grant = useMutation({
    mutationFn: () => api.grantGuest(projectId!, email.trim()),
    onSuccess: () => {
      push("success", "Guest added", email);
      setEmail("");
      setError(null);
      setAddOpen(false);
      invalidate();
    },
    onError: (err) =>
      setError(err instanceof ApiError ? err.message : "Could not add the guest."),
  });

  const revoke = useMutation({
    mutationFn: (userId: string) => api.revokeGuest(projectId!, userId),
    onSuccess: () => {
      push("info", "Guest access revoked");
      invalidate();
    },
    onError: (err) =>
      push("error", "Could not revoke access", err instanceof Error ? err.message : undefined),
  });

  const confirmRevoke = (name: string, userId: string) => {
    if (window.confirm(`Revoke ${name}'s access to this project?`)) {
      revoke.mutate(userId);
    }
  };

  if (project.isError) {
    return (
      <div className="mx-auto max-w-2xl p-6">
        <EmptyState
          icon={<Users className="h-8 w-8" />}
          title="Project unavailable"
          description="It may have been archived or you may lack access."
        />
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-2xl p-6">
      <header className="mb-5 flex items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">
            Guests — {project.data?.name ?? "…"}
          </h1>
          <p className="text-xs text-muted-foreground">
            Guests get read-only access to this project only. They are not team
            members.
          </p>
        </div>
        <Button size="sm" onClick={() => setAddOpen(true)} data-testid="add-guest">
          <UserPlus /> Add guest
        </Button>
      </header>

      {guests.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : (guests.data?.items.length ?? 0) === 0 ? (
        <EmptyState
          icon={<Users className="h-8 w-8" />}
          title="No guests yet"
          description="Grant read-only access to an existing user by email."
          action={
            <Button onClick={() => setAddOpen(true)}>
              <UserPlus /> Add guest
            </Button>
          }
        />
      ) : (
        <ul className="overflow-hidden rounded-lg border" data-testid="guest-list">
          {guests.data!.items.map((guest) => (
            <li
              key={guest.user.id}
              className="flex items-center gap-3 border-b px-4 py-2.5 last:border-0"
            >
              <Avatar name={guest.user.display_name} size="md" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">
                  {guest.user.display_name}
                  <Badge variant="muted" className="ml-2">
                    {guest.role}
                  </Badge>
                </p>
                <p className="truncate text-2xs text-muted-foreground">
                  {guest.user.email} · granted {timeAgo(guest.created_at)}
                  {guest.granted_by?.display_name
                    ? ` by ${guest.granted_by.display_name}`
                    : ""}
                </p>
              </div>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Revoke ${guest.user.display_name}`}
                title="Revoke access"
                onClick={() => confirmRevoke(guest.user.display_name, guest.user.id)}
                className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                data-testid={`revoke-${guest.user.id}`}
              >
                <Trash2 />
              </Button>
            </li>
          ))}
        </ul>
      )}

      <p className="mt-4 text-2xs text-muted-foreground">
        <Link to={`/projects/${projectId}`} className="text-primary hover:underline">
          ← Back to project
        </Link>
      </p>

      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add project guest</DialogTitle>
            <DialogDescription>
              The user must already have a Tasker account. They get read-only
              access to this project only — never the team.
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
              setError(null);
              grant.mutate();
            }}
            noValidate
            data-testid="add-guest-form"
          >
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="guest-email">Email</Label>
              <Input
                id="guest-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="guest@company.com"
                autoFocus
                data-testid="guest-email-input"
              />
            </div>
            {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
            <DialogFooter>
              <Button variant="outline" onClick={() => setAddOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={grant.isPending} data-testid="guest-submit">
                {grant.isPending ? "Adding…" : "Grant access"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}
