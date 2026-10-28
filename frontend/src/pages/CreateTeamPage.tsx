import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input, Label, Textarea } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";

export function CreateTeamPage() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { push } = useToast();
  const [open] = useState(true);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.createTeam(name, description || undefined),
    onSuccess: (team) => {
      push("success", "Team created", team.name);
      void qc.invalidateQueries({ queryKey: ["teams"] });
      navigate(`/?team=${team.id}`, { replace: true });
    },
    onError: (err) => setError(err instanceof ApiError ? err.message : "Could not create the team."),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) navigate(-1);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create a team</DialogTitle>
          <DialogDescription>
            You will be the team Owner and can invite teammates afterwards.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!name.trim()) {
              setError("Team name is required.");
              return;
            }
            setError(null);
            create.mutate();
          }}
          noValidate
          data-testid="create-team-form"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="team-new-name">Name</Label>
            <Input
              id="team-new-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Acme Inc"
              autoFocus
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="team-new-description">Description</Label>
            <Textarea
              id="team-new-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What does this team work on?"
              rows={3}
            />
          </div>
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => navigate(-1)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending} data-testid="create-team-submit">
              {create.isPending ? "Creating…" : "Create team"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
