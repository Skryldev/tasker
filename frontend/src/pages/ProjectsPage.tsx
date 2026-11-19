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
import { useTranslation } from "react-i18next";
import { useToast } from "@/components/ui/toast";
import { EmptyState } from "@/components/app/EmptyState";
import { useWorkspace } from "@/lib/workspace";
import { timeAgo } from "@/lib/utils";

export function ProjectsPage() {
  const { t } = useTranslation("common");
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
        title={t("projects.noTeam")}
        description={t("projects.noTeamDesc")}
      />
    );
  }

  return (
    <div className="mx-auto max-w-5xl p-6">
      <header className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">{t("nav.projects")}</h1>
          <p className="text-xs text-muted-foreground">{t("projects.subtitle")}</p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant={includeArchived ? "secondary" : "ghost"}
            size="sm"
            onClick={() => setIncludeArchived((v) => !v)}
            aria-pressed={includeArchived}
          >
            <Archive /> {t("projects.archived")}
          </Button>
          <Button size="sm" onClick={() => setCreateOpen(true)} data-testid="create-project">
            <Plus /> {t("projects.new")}
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
          title={includeArchived ? t("projects.noneAtAll") : t("projects.noneActive")}
          description={t("projects.createFirst")}
          action={
            <Button onClick={() => setCreateOpen(true)}>
              <Plus /> {t("projects.new")}
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
  const { t } = useTranslation("common");
  const { push } = useToast();
  const qc = useQueryClient();
  const archived = project.status === "archived";
  const setArchived = useMutation({
    mutationFn: () => api.setProjectArchived(project.id, !archived),
    onSuccess: () => {
      push("success", archived ? t("projects.restored") : t("projects.archivedToast"));
      void qc.invalidateQueries({ queryKey: ["projects"] });
    },
    onError: (error) =>
      push("error", t("projects.changeFailed"), error instanceof Error ? error.message : undefined),
  });

  return (
    <li className="group relative">
      <Link
        to={`/projects/${project.id}`}
        className={`flex h-full flex-col gap-2 rounded-sg border bg-card p-4 shadow-sm transition-all hover:border-primary/40 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
          archived ? "opacity-60" : ""
        }`}
      >
        <div className="flex items-center gap-2">
          <span className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/10 text-primary">
            <FolderKanban className="h-4 w-4" />
          </span>
          <h2 className="truncate text-sm font-semibold">{project.name}</h2>
          {archived ? <Badge variant="muted" className="ms-auto">{t("projects.archived")}</Badge> : null}
        </div>
        <p className="line-clamp-2 min-h-[2rem] text-xs text-muted-foreground">
          {project.description || t("projects.noDescription")}
        </p>
        <div className="mt-auto flex items-center justify-between text-2xs text-muted-foreground">
          <span>{t("projects.openTasks", { count: project.open_task_count ?? 0 })}</span>
          <span>{t("projects.updated", { when: timeAgo(project.updated_at) })}</span>
        </div>
      </Link>
      <div className="absolute end-2 top-2 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-within:opacity-100">
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
  const { t } = useTranslation("common");
  const { push } = useToast();
  const qc = useQueryClient();
  const [deleteOpen, setDeleteOpen] = useState(false);

  const remove = useMutation({
    mutationFn: () => api.deleteProject(project.id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["projects"] });
      push("success", t("projects.deleted"), t("projects.undoAvailable"), {
        action: {
          label: t("projects.undo"),
          onClick: () => {
            api
              .restoreProject(project.id)
              .then(() => {
                void qc.invalidateQueries({ queryKey: ["projects"] });
                push("success", t("projects.restored"));
              })
              .catch((error) =>
                push("error", t("projects.restoreFailed"), error instanceof Error ? error.message : undefined),
              );
          },
        },
        duration: 5000,
      });
    },
    onError: (error) =>
      push("error", t("projects.deleteFailed"), error instanceof Error ? error.message : undefined),
  });

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("projects.actionsAria", { name: project.name })}
            data-testid={`project-menu-${project.id}`}
          >
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-44">
          <DropdownMenuItem onSelect={onToggleArchive}>
            {archived ? <ArchiveRestore /> : <Archive />}
            {archived ? t("projects.restore") : t("projects.archive")}
          </DropdownMenuItem>
          <DropdownMenuItem destructive onSelect={() => setDeleteOpen(true)} data-testid="delete-project">
            <Trash2 /> {t("projects.delete")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={t("projects.deleteConfirmTitle")}
        description={t("projects.deleteConfirmDesc", { name: project.name })}
        confirmLabel={t("projects.delete")}
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
  const { t } = useTranslation("common");
  const { push } = useToast();
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.createProject(teamId, { name, description: description || undefined }),
    onSuccess: () => {
      push("success", t("projects.created"));
      void qc.invalidateQueries({ queryKey: ["projects", teamId] });
      setName("");
      setDescription("");
      onOpenChange(false);
    },
    onError: (err) =>
      setError(err instanceof ApiError ? err.message : t("projects.createFailed")),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("projects.new")}</DialogTitle>
          <DialogDescription>{t("projects.newDesc")}</DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!name.trim()) {
              setError(t("projects.nameRequired"));
              return;
            }
            setError(null);
            create.mutate();
          }}
          noValidate
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-name">{t("projects.nameLabel")}</Label>
            <Input
              id="project-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("projects.namePlaceholder")}
              autoFocus
              data-testid="project-name-input"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="project-description">{t("projects.descriptionLabel")}</Label>
            <Textarea
              id="project-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("projects.descriptionPlaceholder")}
              rows={3}
            />
          </div>
          {error ? <p className="text-2xs text-destructive" role="alert">{error}</p> : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("actions.cancel")}
            </Button>
            <Button type="submit" disabled={create.isPending} data-testid="project-submit">
              {create.isPending ? t("projects.creating") : t("projects.create")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
