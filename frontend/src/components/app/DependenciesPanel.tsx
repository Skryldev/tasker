import { Suspense, lazy, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { Link2, ListTree, Plus, Trash2, Users, Network } from "lucide-react";
import { api, ApiError, type DependencyRef2, type DependencyType, type Task } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useToast } from "@/components/ui/toast";
import { cn, STATUS_LABELS, type TaskStatus } from "@/lib/utils";

/**
 * Dependency panel (Phase 14 WS6, ADR-0023): grouped edges by type, a
 * searchable command-menu picker with type selection and opt-in
 * auto-scheduling, an At Risk badge for overdue dependencies, and a lazy
 * graph modal for the blocks-only visualization.
 */

export const DEPENDENCY_TYPES: {
  id: DependencyType;
  label: string;
  description: string;
  color: string;
}[] = [
  {
    id: "blocks",
    label: "Blocks",
    description: "Must finish before this task (prevents cycles, feeds scheduling).",
    color: "#ef4444",
  },
  {
    id: "relates_to",
    label: "Relates to",
    description: "Informational link; no scheduling effect.",
    color: "#3b82f6",
  },
  {
    id: "duplicates",
    label: "Duplicates",
    description: "Tracks the same work as another task.",
    color: "#8b5cf6",
  },
  {
    id: "causes",
    label: "Causes",
    description: "This task's outcome drives the other one.",
    color: "#f59e0b",
  },
];

const TYPE_META: Record<DependencyType, (typeof DEPENDENCY_TYPES)[number]> = {
  blocks: DEPENDENCY_TYPES[0]!,
  relates_to: DEPENDENCY_TYPES[1]!,
  duplicates: DEPENDENCY_TYPES[2]!,
  causes: DEPENDENCY_TYPES[3]!,
};

/** A dependency is At Risk when it is overdue and not finished. */
function isAtRisk(ref: DependencyRef2, today: string): boolean {
  return Boolean(
    ref.due_date &&
      ref.due_date < today &&
      ref.status !== "closed" &&
      ref.status !== "resolved",
  );
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

function DependencyCard({
  dep,
  onRemove,
  direction,
}: {
  dep: DependencyRef2;
  onRemove?: () => void;
  direction: "blocked_by" | "blocking";
}) {
  const meta = TYPE_META[dep.type] ?? TYPE_META.blocks;
  const atRisk = isAtRisk(dep, todayIso());
  const closed = dep.status === "closed";
  return (
    <div
      className="group flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-sm"
      data-testid={`dep-${dep.task_id}`}
      style={{ borderLeft: `3px solid ${meta.color}` }}
    >
      <span
        className={cn(
          "h-2 w-2 shrink-0 rounded-full",
          closed ? "bg-status-closed" : "bg-status-in-progress",
        )}
        aria-label={closed ? "closed" : "not closed"}
      />
      <span className={cn("min-w-0 truncate", closed && "text-muted-foreground line-through")}>
        {dep.title}
      </span>
      <span
        className="shrink-0 rounded-full px-1.5 py-0.5 text-2xs font-medium"
        style={{ backgroundColor: `${meta.color}1f`, color: meta.color }}
        title={meta.description}
      >
        {meta.label}
      </span>
      {atRisk ? (
        <Badge className="shrink-0 bg-warning/15 text-warning" data-testid="dep-at-risk">
          At Risk
        </Badge>
      ) : null}
      <span className="ml-auto shrink-0 text-2xs text-muted-foreground">
        {direction === "blocked_by" ? "must finish first" : "waits on this"}
        {" · "}
        {STATUS_LABELS[dep.status as TaskStatus] ?? dep.status}
      </span>
      {onRemove ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label="Remove dependency"
          className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
          data-testid={`dep-remove-${dep.dep_id}`}
        >
          <Trash2 className="h-3 w-3" />
        </button>
      ) : null}
    </div>
  );
}

/** Searchable command-menu picker: query + arrow/enter keyboard support. */
function TaskPicker({
  candidates,
  value,
  onPick,
  placeholder,
  testId,
}: {
  candidates: { id: string; title: string; status: string }[];
  value: string | null;
  onPick: (id: string) => void;
  placeholder: string;
  testId: string;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return candidates
      .filter((c) => !q || c.title.toLowerCase().includes(q))
      .slice(0, 8);
  }, [candidates, query]);
  return (
    <div className="flex flex-col gap-1.5" data-testid={testId}>
      <input
        ref={inputRef}
        autoFocus
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setActive((a) => Math.min(a + 1, filtered.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setActive((a) => Math.max(a - 1, 0));
          } else if (e.key === "Enter" && filtered[active]) {
            e.preventDefault();
            onPick(filtered[active].id);
          } else if (e.key === "Escape") {
            setQuery("");
          }
        }}
        placeholder={placeholder}
        aria-label="Search for a task"
        className="h-8 w-full rounded-md border bg-background px-2.5 text-xs outline-none focus:ring-1 focus:ring-primary"
        data-testid={`${testId}-search`}
      />
      <ul
        className="max-h-40 overflow-y-auto rounded-md border scrollbar-thin"
        role="listbox"
        aria-label="Task candidates"
      >
        {filtered.length === 0 ? (
          <li className="p-2 text-center text-2xs text-muted-foreground">No matching tasks.</li>
        ) : (
          filtered.map((c, i) => (
            <li key={c.id} role="none">
              <button
                type="button"
                role="option"
                aria-selected={value === c.id}
                onClick={() => onPick(c.id)}
                onMouseEnter={() => setActive(i)}
                className={cn(
                  "flex w-full items-center justify-between gap-2 px-2.5 py-1.5 text-left text-xs hover:bg-accent",
                  i === active && "bg-accent",
                )}
              >
                <span className="min-w-0 truncate">{c.title}</span>
                <span className="shrink-0 text-2xs text-muted-foreground">
                  {STATUS_LABELS[c.status as TaskStatus] ?? c.status}
                </span>
              </button>
            </li>
          ))
        )}
      </ul>
    </div>
  );
}

/** The graph modal is code-split: the eager task views never carry it. */
const DependencyGraphModal = lazy(() =>
  import("./DependencyGraphModal").then((m) => ({ default: m.DependencyGraphModal })),
);

export function DependenciesPanel({ task }: { task: Task }) {
  const qc = useQueryClient();
  const { push } = useToast();
  const [addOpen, setAddOpen] = useState(false);
  const [candidate, setCandidate] = useState<string | null>(null);
  const [depType, setDepType] = useState<DependencyType>("blocks");
  const [autoSchedule, setAutoSchedule] = useState(false);
  const [graphOpen, setGraphOpen] = useState(false);

  const deps = useQuery({
    queryKey: ["dependencies", task.id],
    queryFn: () => api.listDependencies(task.id),
  });

  const candidates = useQuery({
    queryKey: ["tasks", task.project_id, "all-for-deps"],
    queryFn: () => api.listTasks(task.project_id, { page_size: 100 }),
    enabled: addOpen,
  });

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["dependencies", task.id] });
  };

  const add = async () => {
    if (!candidate || candidate === task.id) return;
    try {
      await api.createDependency(task.id, {
        from_task_id: candidate,
        dep_type: depType,
        auto_schedule: depType === "blocks" ? autoSchedule : false,
      });
      setAddOpen(false);
      setCandidate(null);
      setAutoSchedule(false);
      refresh();
      push("success", "Dependency added");
    } catch (error) {
      push(
        "error",
        "Could not add dependency",
        error instanceof ApiError ? error.message : "Check the task selection.",
      );
    }
  };

  const remove = async (depId: string) => {
    try {
      await api.removeDependency(task.id, depId);
      refresh();
    } catch (error) {
      push("error", "Could not remove dependency", error instanceof Error ? error.message : undefined);
    }
  };

  const blockedBy = deps.data?.blocked_by ?? [];
  const blocking = deps.data?.blocking ?? [];
  const canManage = deps.data?.can_manage ?? false;
  const openBlockers = blockedBy.filter(
    (b) => b.type === "blocks" && b.status !== "closed" && b.status !== "resolved",
  ).length;

  if (deps.isLoading) {
    return <div className="h-16 animate-pulse rounded bg-muted" />;
  }

  return (
    <section aria-label="Dependencies" className="mt-5" data-testid="dependencies-panel">
      <div className="mb-2 flex items-center justify-between">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold">
          <Link2 className="h-3.5 w-3.5" /> Dependencies
          {openBlockers > 0 ? (
            <Badge className="ml-1 bg-destructive/10 text-destructive" data-testid="blocked-count">
              {openBlockers} open blocker{openBlockers === 1 ? "" : "s"}
            </Badge>
          ) : null}
        </h3>
        <div className="flex items-center gap-1">
          {blockedBy.length + blocking.length > 0 ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setGraphOpen(true)}
              data-testid="show-dependency-graph"
            >
              <Network /> Graph
            </Button>
          ) : null}
          {canManage ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setAddOpen((v) => !v)}
              data-testid="add-dependency"
            >
              <Plus /> Add
            </Button>
          ) : null}
        </div>
      </div>

      {addOpen ? (
        <div className="mb-3 flex flex-col gap-2 rounded-md border p-3" data-testid="dependency-picker">
          <TaskPicker
            candidates={(candidates.data?.items ?? [])
              .filter((t) => t.id !== task.id)
              .map((t) => ({ id: t.id, title: t.title, status: t.status }))}
            value={candidate}
            onPick={setCandidate}
            placeholder="Search for a task that this one relates to…"
            testId="dependency-task-picker"
          />
          <div className="flex flex-wrap items-center gap-1.5" role="radiogroup" aria-label="Dependency type">
            {DEPENDENCY_TYPES.map((t) => (
              <button
                key={t.id}
                type="button"
                role="radio"
                aria-checked={depType === t.id}
                title={t.description}
                onClick={() => setDepType(t.id)}
                className={cn(
                  "rounded-full border px-2 py-0.5 text-2xs font-medium transition-colors",
                  depType === t.id ? "border-transparent" : "text-muted-foreground hover:bg-accent",
                )}
                style={
                  depType === t.id
                    ? { backgroundColor: `${t.color}1f`, color: t.color, borderColor: `${t.color}55` }
                    : undefined
                }
                data-testid={`dep-type-${t.id}`}
              >
                {t.label}
              </button>
            ))}
          </div>
          {depType === "blocks" ? (
            <label className="flex items-center gap-2 text-2xs text-muted-foreground">
              <input
                type="checkbox"
                checked={autoSchedule}
                onChange={(e) => setAutoSchedule(e.target.checked)}
                className="h-3.5 w-3.5 rounded border-input accent-primary"
                data-testid="auto-schedule-toggle"
              />
              Auto-schedule: pull this task's due date forward when the dependency's moves later
            </label>
          ) : null}
          <div className="flex justify-end">
            <Button
              size="sm"
              onClick={() => void add()}
              disabled={!candidate}
              data-testid="dependency-submit"
            >
              Link
            </Button>
          </div>
        </div>
      ) : null}

      {blockedBy.length === 0 && blocking.length === 0 ? (
        <p className="text-xs text-muted-foreground">No dependencies.</p>
      ) : (
        <div className="flex flex-col gap-3">
          {blockedBy.length > 0 ? (
            <div>
              <p className="mb-1 text-2xs uppercase tracking-wide text-muted-foreground">
                Depends on ({blockedBy.length})
              </p>
              <div className="flex flex-col gap-1">
                {blockedBy.map((b) => (
                  <DependencyCard
                    key={b.dep_id}
                    dep={b}
                    direction="blocked_by"
                    onRemove={canManage ? () => void remove(b.dep_id) : undefined}
                  />
                ))}
              </div>
            </div>
          ) : null}
          {blocking.length > 0 ? (
            <div>
              <p className="mb-1 text-2xs uppercase tracking-wide text-muted-foreground">
                Other tasks waiting on this ({blocking.length})
              </p>
              <div className="flex flex-col gap-1">
                {blocking.map((b) => (
                  <DependencyCard key={b.dep_id} dep={b} direction="blocking" />
                ))}
              </div>
            </div>
          ) : null}
        </div>
      )}

      {graphOpen ? (
        <Suspense fallback={null}>
          <DependencyGraphModal task={task} onClose={() => setGraphOpen(false)} />
        </Suspense>
      ) : null}
    </section>
  );
}

/** Compact navigation card used on project pages to reach workload & guests. */
export function ProjectToolsLinks({ projectId }: { projectId: string }) {
  return (
    <div className="flex gap-2">
      <Link
        to={`/projects/${projectId}?view=workload`}
        className="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent"
      >
        <ListTree className="h-3.5 w-3.5" /> Workload
      </Link>
      <Link
        to={`/projects/${projectId}/guests`}
        className="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent"
      >
        <Users className="h-3.5 w-3.5" /> Guests
      </Link>
    </div>
  );
}
