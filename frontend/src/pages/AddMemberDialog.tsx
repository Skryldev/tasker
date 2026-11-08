import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { roleLabel } from "@/lib/utils";
import { useToast } from "@/components/ui/toast";

/**
 * Add member (Phase 12.5 Bug 2): a searchable picker over EXISTING instance
 * users. Team membership no longer creates accounts — an unknown email gets
 * an actionable pointer to an Owner (ADR-0014). Keyboard: arrows move, Enter
 * assigns.
 */

const ROLES = ["member", "manager", "admin"] as const;

export function AddMemberDialog({
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
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [role, setRole] = useState<string>("member");
  const [activeIndex, setActiveIndex] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setDebounced("");
      setRole("member");
      setActiveIndex(0);
      setError(null);
    }
  }, [open]);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query]);

  // Debounced search over eligible instance users (server-side, paginated).
  const eligible = useQuery({
    queryKey: ["eligible-members", teamId, debounced],
    queryFn: () => adminApi.eligibleMembers(teamId, { q: debounced || undefined, page_size: 20 }),
    enabled: open,
    placeholderData: (prev) => prev,
  });

  const add = useMutation({
    mutationFn: (email: string) =>
      api.addMember(teamId, { email, role }),
    onSuccess: (_member, email) => {
      push("success", "Member added", email);
      void qc.invalidateQueries({ queryKey: ["members", teamId] });
      void qc.invalidateQueries({ queryKey: ["eligible-members", teamId] });
      onOpenChange(false);
    },
    onError: (err) =>
      setError(err instanceof ApiError ? err.message : "Could not add the member."),
  });

  const items = eligible.data?.items ?? [];

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIndex((i) => Math.min(items.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(0, i - 1));
    } else if (e.key === "Enter" && items[activeIndex]) {
      e.preventDefault();
      add.mutate(items[activeIndex].email);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add member</DialogTitle>
          <DialogDescription>
            Pick an existing Tasker account. Accounts are created by an Owner on the
            Users page — team membership is a separate assignment.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3" data-testid="add-member-form">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="member-search-input">Find a user</Label>
            <Input
              id="member-search-input"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onKeyDown}
              placeholder="Search by name, email or username…"
              autoFocus
              role="combobox"
              aria-expanded
              aria-controls="eligible-members-list"
              data-testid="member-search-input"
            />
          </div>
          <ul
            id="eligible-members-list"
            role="listbox"
            aria-label="Eligible users"
            className="flex max-h-56 flex-col overflow-y-auto rounded-md border p-1 scrollbar-thin"
            data-testid="eligible-members-list"
          >
            {eligible.isLoading || query !== debounced ? (
              <li className="flex flex-col gap-1.5 p-1.5" aria-live="polite">
                <Skeleton className="h-7 w-full" />
                <Skeleton className="h-7 w-4/5" />
              </li>
            ) : eligible.isError ? (
              <li className="flex items-center justify-between gap-2 p-2 text-xs text-muted-foreground" role="alert">
                Could not load users.
                <Button variant="outline" size="sm" onClick={() => void eligible.refetch()}>
                  Retry
                </Button>
              </li>
            ) : items.length === 0 ? (
              <li className="p-3 text-center text-xs text-muted-foreground" aria-live="polite" data-testid="eligible-empty">
                No existing Tasker user matches this search.{" "}
                <span className="underline decoration-dotted">Ask an Owner to create the account.</span>
              </li>
            ) : (
              items.map((u, i) => (
                <li key={u.id} role="none">
                  <button
                    type="button"
                    role="option"
                    aria-selected={i === activeIndex}
                    onClick={() => add.mutate(u.email)}
                    onMouseEnter={() => setActiveIndex(i)}
                    className={`flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm ${i === activeIndex ? "bg-accent" : ""}`}
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
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="member-role">Role</Label>
            <Select value={role} onValueChange={setRole}>
              <SelectTrigger id="member-role" data-testid="member-role">
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
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}
