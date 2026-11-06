import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Search } from "lucide-react";
import { api } from "@/api/client";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { THEMES, themeById, useTheme } from "@/lib/theme";
import { useWorkspace } from "@/lib/workspace";
import { cn } from "@/lib/utils";

/**
 * Command palette (Phase 10.5 §6.1): one dialog for navigation, creation,
 * server-side task search, and theme switching. The component is
 * lazy-loaded from the AppShell so the initial bundle carries nothing.
 *
 * Dependency note: the repo has no shadcn `Command` primitive (no cmdk);
 * this palette is ~150 lines of dialog + listbox instead of a new runtime
 * dependency, per the phase's dependency rule.
 */

interface CommandItem {
  id: string;
  label: string;
  hint?: string;
  section: string;
  keywords?: string;
  run: () => void;
}

export function CommandPalette({ open, onOpenChange }: { open: boolean; onOpenChange: (v: boolean) => void }) {
  const navigate = useNavigate();
  const { teamId, teams } = useWorkspace();
  const { setTheme } = useTheme();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const close = () => onOpenChange(false);

  const activeTeam = teamId ?? teams.data?.items[0]?.id;
  const firstProject = useQuery({
    queryKey: ["projects", activeTeam, "palette"],
    queryFn: () => api.listProjects(activeTeam!),
    enabled: open && Boolean(activeTeam),
    staleTime: 60_000,
  });

  // Server-side task search once the query is at least two characters.
  const [debounced, setDebounced] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query]);
  const taskResults = useQuery({
    queryKey: ["my-tasks", { palette: debounced }],
    queryFn: () => api.myTasks({ q: debounced, page_size: 6 }),
    enabled: open && debounced.length >= 2,
  });

  const commands = useMemo<CommandItem[]>(() => {
    const items: CommandItem[] = [
      {
        id: "go-tasks",
        label: "My Tasks",
        section: "Navigate",
        keywords: "tasks home",
        run: () => navigate("/"),
      },
      {
        id: "go-projects",
        label: "Projects",
        section: "Navigate",
        keywords: "projects",
        run: () => navigate(activeTeam ? `/projects?team=${activeTeam}` : "/projects"),
      },
      {
        id: "go-goals",
        label: "Goals",
        section: "Navigate",
        keywords: "goals okr",
        run: () => activeTeam && navigate(`/teams/${activeTeam}/goals`),
      },
      {
        id: "go-workload",
        label: "Workload",
        section: "Navigate",
        keywords: "workload",
        run: () => activeTeam && navigate(`/teams/${activeTeam}/workload`),
      },
      {
        id: "go-capacity",
        label: "Capacity",
        section: "Navigate",
        keywords: "capacity utilization",
        run: () => activeTeam && navigate(`/teams/${activeTeam}/capacity`),
      },
      {
        id: "go-settings",
        label: "Team Settings",
        section: "Navigate",
        keywords: "settings team",
        run: () => activeTeam && navigate(`/teams/${activeTeam}/settings?team=${activeTeam}`),
      },
      {
        id: "go-account",
        label: "Account",
        section: "Navigate",
        keywords: "profile account",
        run: () => navigate("/account"),
      },
      {
        id: "new-task",
        label: "New task…",
        section: "Create",
        hint: activeTeam && firstProject.data?.items.length ? undefined : "needs a project",
        keywords: "create task add",
        run: () =>
          navigate(
            firstProject.data?.items[0]
              ? `/projects/${firstProject.data.items[0].id}?team=${activeTeam}&new=task`
              : "/projects",
          ),
      },
      {
        id: "new-project",
        label: "New project…",
        section: "Create",
        keywords: "create project add",
        run: () => navigate(activeTeam ? `/projects?team=${activeTeam}&new=project` : "/projects"),
      },
    ];
    if (activeTeam) {
      void activeTeam;
    }
    // Themes.
    for (const definition of THEMES) {
      items.push({
        id: `theme-${definition.id}`,
        label: `Theme: ${definition.label}`,
        section: "Theme",
        keywords: `theme appearance ${definition.id}`,
        run: () => setTheme(definition.id),
      });
    }
    return items;
  }, [navigate, activeTeam, firstProject.data, setTheme]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    // Subsequence fuzzy match on label + keywords; contiguous matches first.
    const scored = commands
      .map((c) => ({ c, score: fuzzyScore(q, `${c.label} ${c.keywords ?? ""}`) }))
      .filter((x) => x.score > 0);
    scored.sort((a, b) => b.score - a.score);
    return scored.map((x) => x.c);
  }, [commands, query]);

  const taskHits =
    debounced.length >= 2
      ? (taskResults.data?.items ?? []).slice(0, 6)
      : [];

  useEffect(() => {
    setActive(0);
  }, [query, open]);

  const flatCount = filtered.length + taskHits.length;

  const runAt = (index: number) => {
    const command = filtered[index];
    if (index < filtered.length && command) {
      command.run();
      close();
      return;
    }
    const hit = taskHits[index - filtered.length];
    if (hit) {
      navigate(`/tasks/${hit.id}`);
      close();
    }
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => (flatCount === 0 ? 0 : (i + 1) % flatCount));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => (flatCount === 0 ? 0 : (i - 1 + flatCount) % flatCount));
    } else if (e.key === "Enter") {
      e.preventDefault();
      runAt(active);
    }
  };

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="top-[15%] max-w-lg translate-y-0 gap-0 p-0"
        onKeyDown={onKeyDown}
        aria-describedby={undefined}
      >
        <DialogTitle className="sr-only">Command palette</DialogTitle>
        <div className="flex items-center gap-2 border-b px-3">
          <Search className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search tasks or type a command…"
            aria-label="Search commands"
            autoFocus
            data-testid="palette-input"
            className="h-11 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>
        <div
          ref={listRef}
          role="listbox"
          aria-label="Commands"
          className="max-h-80 overflow-y-auto scrollbar-thin p-2"
          data-testid="palette-list"
        >
          {flatCount === 0 ? (
            <p className="px-3 py-6 text-center text-xs text-muted-foreground">
              No matching commands or tasks.
            </p>
          ) : null}
          {filtered.map((item, i) => (
            <PaletteRow
              key={item.id}
              index={i}
              active={active === i}
              label={item.label}
              hint={item.hint}
              section={
                i === 0 || filtered[i - 1]?.section !== item.section
                  ? item.section
                  : undefined
              }
              onHover={() => setActive(i)}
              onRun={() => runAt(i)}
            />
          ))}
          {taskHits.map((task, i) => {
            const index = filtered.length + i;
            return (
              <PaletteRow
                key={task.id}
                index={index}
                active={active === index}
                label={task.title}
                hint="Task"
                section={i === 0 ? "Tasks" : undefined}
                onHover={() => setActive(index)}
                onRun={() => runAt(index)}
              />
            );
          })}
        </div>
        <p className="border-t px-3 py-2 text-2xs text-muted-foreground">
          ↑↓ navigate · Enter run · Esc close — press ? anywhere for all shortcuts
        </p>
      </DialogContent>
    </Dialog>
  );
}

function PaletteRow({
  index,
  active,
  label,
  hint,
  section,
  onHover,
  onRun,
}: {
  index: number;
  active: boolean;
  label: string;
  hint?: string;
  section?: string;
  onHover: () => void;
  onRun: () => void;
}) {
  return (
    <>
      {section ? (
        <p className="px-3 pb-1 pt-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground">
          {section}
        </p>
      ) : null}
      <div
        role="option"
        aria-selected={active}
        data-index={index}
        tabIndex={-1}
        onMouseEnter={onHover}
        onMouseDown={(e) => e.preventDefault()}
        onClick={onRun}
        className={cn(
          "flex cursor-pointer items-center justify-between gap-2 rounded-md px-3 py-2 text-sm transition-colors duration-100",
          active ? "bg-accent text-accent-foreground" : "text-foreground",
        )}
      >
        <span className="truncate">{label}</span>
        {hint ? <span className="shrink-0 text-2xs text-muted-foreground">{hint}</span> : null}
      </div>
    </>
  );
}

/**
 * Lightweight fuzzy score: 0 = no match; higher is better (contiguous runs
 * and earlier hits score higher). Deterministic, allocation-free.
 */
function fuzzyScore(query: string, target: string): number {
  const t = target.toLowerCase();
  let score = 0;
  let ti = 0;
  let run = 0;
  for (const qc of query) {
    const found = t.indexOf(qc, ti);
    if (found === -1) return 0;
    run = found === ti ? run + 1 : 1;
    score += 1 + run * 2 + Math.max(0, 8 - found);
    ti = found + 1;
  }
  return score;
}

/** Resolve a theme id for display (exported for tests). */
export function themeLabel(id: string): string {
  return themeById(id)?.label ?? id;
}
