import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Archive, ArchiveRestore, FolderKanban, MoreHorizontal, Plus, Trash2 } from "lucide-react";
import { api, ApiError, type Project } from "@/api/client";
import { useProjects } from "@/api/hooks";
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
import { Input, Label, Textarea } from "@/components/ui/input";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { useToast } from "@/components/ui/toast";
import { EmptyState } from "@/components/app/EmptyState";
import { useWorkspace } from "@/lib/workspace";
import { timeAgo } from "@/lib/utils";

export function ProjectsPage() {
  const { teamId } = useWorkspace();
  const [includeArchived, setIncludeArchived] = useState(false);
  const [createOpen, setCreateOpen] = useState(
    () => new URLSearchParams(window.location.search).get("new") === "project",
  );
  const projects = useProjects(teamId, includeArchived);

  if (!teamId) {
    return (
      <EmptyState
        icon={<FolderKanban className="h-8 w-8" />}
        title="No team selected"
        description="Create or select a team to see its projects."
      />
    );
  }

  return (
    <div className="mx-auto max-w-5xl p-6">
      <header className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Projects</h1>
          <p className="text-xs text-muted-foreground">Work streams inside your team.</p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant={includeArchived ? "secondary" : "ghost"}
            size="sm"
            onClick={() => setIncludeArchived((v) => !v)}
            aria-pressed={includeArchived}
          >
            <Archive /> Archived
          </Button>
          <Button size="sm" onClick={() => setCreateOpen(true)} data-testid="create-project">
            <Plus /> New project
          </Button>
        </div>
      </header>

      {projects.isLoading ? (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-28 w-full" />
          ))}
        </div>
      ) : (projects.data?.items.length ?? 0) === 0 ? (
        <EmptyState
          icon={<FolderKanban className="h-8 w-8" />}
          title={includeArchived ? "No projects at all" : "No active projects"}
          description="Create the first project to start tracking work."
          action={
            <Button onClick={() => setCreateOpen(true)}>
              <Plus /> New project
            </Button>
          }
        />
      ) : (
        <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3" data-testid="project-list">
          {projects.data!.items.map((project) => (
            <ProjectCard key={project.id} project={project} />
          ))}
        </ul>
      )}

      <CreateProjectDialog open={createOpen} onOpenChange={setCreateOpen} teamId={teamId} />
    </div>
  );
}

function ProjectCard({ project }: { project: Project }) {
  const { push } = useToast();
  const qc = useQueryClient();
  const archived = project.status === "archived";
  const setArchived = useMutation({
    mutationFn: () => api.setProjectArchived(project.id, !archived),
    onSuccess: () => {
      push("success", archived ? "Project restored" : "Project archived");
      void qc.invalidateQueries({ queryKey: ["projects"] });
    },
    onError: (error) =>
      push("error", "Could not change project", error instanceof Error ? error.message : undefined),
  });

  return (
    <li className="group relative">
      <Link
        to={`/projects/${project.id}`}
        className={`flex h-full flex-col gap-2 rounded-lg border bg-card p-4 shadow-sm transition-all hover:border-primary/40 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
          archived ? "opacity-60" : ""
        }`}
      >
        <div className="flex items-center gap-2">
          <span className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/10 text-primary">
            <FolderKanban className="h-4 w-4" />
          </span>
          <h2 className="truncate text-sm font-semibold">{project.name}</h2>
          {archived ? <Badge variant="muted" className="ml-auto">Archived</Badge> : null}
        </div>
        <p className="line-clamp-2 min-h-[2rem] text-xs text-muted-foreground">
          {project.description || "No description"}
        </p>
        <div className="mt-auto flex items-center justify-between text-2xs text-muted-foreground">
          <span>{project.open_task_count ?? 0} open tasks</span>
          <span>Updated {timeAgo(project.updated_at)}</span>
        </div>
      </Link>
      <div className="absolute right-2 top-2 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-within:opacity-100">
        <ProjectCardMenu
          project={project}
          archived={archived}
          onToggleArchive={() => setArchived.mutate()}
        />
      </div>
    </li>
  );
}

/** Card overflow menu: archive (reversible) and delete (soft, undoable). */
function ProjectCardMenu({
  project,
  archived,
  onToggleArchive,
}: {
  project: Project;
  archived: boolean;
  onToggleArchive: () => void;
}) {
  const { push } = useToast();
  const qc = useQueryClient();
  const [deleteOpen, setDeleteOpen] = useState(false);

  const remove = useMutation({
    mutationFn: () => api.deleteProject(project.id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["projects"] });
      push("success", "Project deleted", "Undo is available for a few seconds.", {
        action: {
          label: "Undo",
          onClick: () => {
            api
              .restoreProject(project.id)
              .then(() => {
                void qc.invalidateQueries({ queryKey: ["projects"] });
                push("success", "Project restored");
              })
              .catch((error) =>
                push("error", "Could not restore", error instanceof Error ? error.message : undefined),
              );
          },
        },
        duration: 5000,
      });
    },
    onError: (error) =>
      push("error", "Could not delete project", error instanceof Error ? error.message : undefined),
  });

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Actions for ${project.name}`}
            data-testid={`project-menu-${project.id}`}
          >
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-44">
          <DropdownMenuItem onSelect={onToggleArchive}>
            {archived ? <ArchiveRestore /> : <Archive />}
            {archived ? "Restore project" : "Archive project"}
          </DropdownMenuItem>
          <DropdownMenuItem destructive onSelect={() => setDeleteOpen(true)} data-testid="delete-project">
            <Trash2 /> Delete project
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title="Delete this project?"
        description={`"${project.name}" and its tasks will be hidden from everyone. You can restore it within 30 days from the team's trash, or undo right now.`}
        confirmLabel="Delete project"
        pending={remove.isPending}
        onConfirm={() => {
          setDeleteOpen(false);
          remove.mutate();
        }}
      />
    </>
  );
}

function CreateProjectDialog({
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
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.createProject(teamId, { name, description: description || undefined }),
    onSuccess: () => {
      push("success", "Project created");
      void qc.invalidateQueries({ queryKey: ["projects", teamId] });
      setName("");
      setDescription("");
      onOpenChange(false);
    },
    onError: (err) =>
      setError(err instanceof ApiError ? err.message : "Could not create the project."),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New project</DialogTitle>
          <DialogDescription>Group related tasks into a project.</DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!name.trim()) {
              setError("Project name is required.");
              return;
            }
            setError(null);
            create.mutate();
          }}
          noValidate
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-name">Name</Label>
            <Input
              id="project-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Website relaunch"
              autoFocus
              data-testid="project-name-input"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-description">Description</Label>
            <Textarea
              id="project-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What is this project about?"
              rows={3}
            />
          </div>
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending} data-testid="project-submit">
              {create.isPending ? "Creating…" : "Create project"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
