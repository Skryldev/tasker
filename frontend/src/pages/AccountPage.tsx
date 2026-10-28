import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { KeyRound } from "lucide-react";
import { api, ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { Separator } from "@/components/ui/avatar";
import { useToast } from "@/components/ui/toast";
import { useAuth } from "@/lib/auth";

export function AccountPage() {
  const { user } = useAuth();
  const { push } = useToast();
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const change = useMutation({
    mutationFn: () => api.changePassword(currentPassword, newPassword),
    onSuccess: () => {
      push("success", "Password changed", "Other sessions were signed out.");
      setCurrentPassword("");
      setNewPassword("");
      setFormError(null);
    },
    onError: (err) =>
      setFormError(err instanceof ApiError ? err.message : "Could not change the password."),
  });

  return (
    <div className="mx-auto max-w-lg p-6">
      <h1 className="mb-1 text-lg font-semibold">Account</h1>
      <p className="mb-6 text-xs text-muted-foreground">Your profile and security settings.</p>

      <section aria-label="Profile" className="mb-8 rounded-lg border p-4">
        <h2 className="mb-3 text-sm font-semibold">Profile</h2>
        <dl className="grid grid-cols-[7rem_1fr] gap-y-2 text-sm">
          <dt className="text-muted-foreground">Display name</dt>
          <dd className="font-medium">{user?.display_name}</dd>
          <dt className="text-muted-foreground">Email</dt>
          <dd>{user?.email}</dd>
        </dl>
      </section>

      <Separator className="mb-8" />

      <section aria-label="Change password" className="rounded-lg border p-4">
        <h2 className="mb-1 flex items-center gap-1.5 text-sm font-semibold">
          <KeyRound className="h-4 w-4" /> Change password
        </h2>
        <p className="mb-3 text-xs text-muted-foreground">
          Changing your password signs out all other sessions.
        </p>
        <form
          className="flex max-w-sm flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (newPassword.length < 10) {
              setFormError("New password must be at least 10 characters.");
              return;
            }
            change.mutate();
          }}
          noValidate
          data-testid="change-password-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="current-password">Current password</Label>
            <Input
              id="current-password"
              type="password"
              autoComplete="current-password"
              value={currentPassword}
              onChange={(e) => setCurrentPassword(e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-password">New password</Label>
            <Input
              id="new-password"
              type="password"
              autoComplete="new-password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
            />
          </div>
          {formError ? <p className="text-2xs text-destructive" role="alert">{formError}</p> : null}
          <Button
            type="submit"
            size="sm"
            className="self-start"
            disabled={!currentPassword || !newPassword || change.isPending}
            data-testid="change-password-submit"
          >
            {change.isPending ? "Saving…" : "Change password"}
          </Button>
        </form>
      </section>
    </div>
  );
}
