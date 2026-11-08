import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Search, X } from "lucide-react";
import { api } from "@/api/client";
import { adminApi } from "@/api/admin-client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * Contribution picker (Phase 12.5 Bug 6): a searchable listbox over team
 * projects or tasks with keyboard navigation, loading/empty/error states,
 * and immediate exclusion of already-associated entries.
 *
 * Root cause note: the association API existed since Phase 4, but no UI
 * called it — the "Browse projects" button simply navigated away, which is
 * why the picker appeared dead.
 */

interface PickerItem {
  id: string;
  label: string;
  hint?: string;
}

export function ContributionPicker({
  teamId,
  kind,
  excludedIds,
  onPick,
  label,
}: {
  teamId: string;
  kind: "projects" | "tasks";
  /** Already-associated entries never appear as options. */
  excludedIds: Set<string>;
  onPick: (id: string) => void;
  /** Accessible label ("Add contributing project"). */
  label: string;
}) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const [busyId, setBusyId] = useState<string | null>(null);
  const listRef = useRef<HTMLUListElement>(null);

  // Projects: the team roster is small — client-side filtering; the query
  // loads when the picker opens (lazy).
  const projects = useQuery({
    queryKey: ["projects", teamId],
    queryFn: () => api.listProjects(teamId),
    enabled: open && kind === "projects",
  });

  // Tasks: server-side search (paginated, debounced).
  const [debounced, setDebounced] = useState("");
  useEffect(() => {
    if (kind !== "tasks") return;
    const t = setTimeout(() => setDebounced(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query, kind]);
  const tasks = useQuery({
    queryKey: ["team-task-search", teamId, debounced],
    queryFn: () => adminApi.searchTeamTasks(teamId, { q: debounced || undefined, page_size: 20 }),
    enabled: open && kind === "tasks",
    placeholderData: (prev) => prev,
  });

  const items: PickerItem[] = useMemo(() => {
    if (kind === "projects") {
      return (projects.data?.items ?? [])
        .filter((p) => !excludedIds.has(p.id) && p.status !== "archived")
        .filter((p) => p.name.toLowerCase().includes(query.trim().toLowerCase()))
        .slice(0, 20)
        .map((p) => ({ id: p.id, label: p.name, hint: p.status.replaceAll("_", " ") }));
    }
    return (tasks.data?.items ?? [])
      .filter((t) => !excludedIds.has(t.id))
      .map((t) => ({
        id: t.id,
        label: t.title,
        hint: `${t.project_name} · ${t.status.replaceAll("_", " ")}`,
      }));
  }, [kind, projects.data, tasks.data, excludedIds, query]);

  const loading =
    kind === "projects" ? projects.isLoading : tasks.isLoading || query !== debounced;
  const error = kind === "projects" ? projects.error : tasks.error;
  const retry = () =>
    kind === "projects" ? void projects.refetch() : void tasks.refetch();

  useEffect(() => {
    setActiveIndex(0);
  }, [items.length, open]);

  const pick = async (id: string) => {
    setBusyId(id);
    try {
      await onPick(id);
      setOpen(false);
      setQuery("");
      void qc.invalidateQueries({ queryKey: ["team-task-search"] });
    } finally {
      setBusyId(null);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIndex((i) => Math.min(items.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(0, i - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[activeIndex];
      if (item) void pick(item.id);
    } else if (e.key === "Escape") {
      setOpen(false);
    }
  };

  useEffect(() => {
    const options = listRef.current?.querySelectorAll<HTMLElement>("[role=option]");
    options?.[activeIndex]?.scrollIntoView({ block: "nearest" });
  }, [activeIndex]);

  if (!open) {
    return (
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => setOpen(true)}
        data-testid={`picker-open-${kind}`}
      >
        <Search className="h-3.5 w-3.5" /> {label}
      </Button>
    );
  }

  return (
    <div className="relative" data-testid={`picker-${kind}`}>
      <div className="flex items-center gap-1.5">
        <Input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={kind === "projects" ? "Search projects…" : "Search tasks…"}
          aria-label={label}
          role="combobox"
          aria-expanded
          aria-controls={`picker-list-${kind}`}
          data-testid={`picker-input-${kind}`}
        />
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Close picker"
          onClick={() => setOpen(false)}
        >
          <X />
        </Button>
      </div>
      <ul
        ref={listRef}
        id={`picker-list-${kind}`}
        role="listbox"
        aria-label={label}
        className="absolute z-20 mt-1 max-h-56 w-full overflow-y-auto rounded-md border bg-popover p-1 shadow-md scrollbar-thin"
      >
        {loading ? (
          <li className="flex flex-col gap-1.5 p-1.5" aria-live="polite">
            <Skeleton className="h-7 w-full" />
            <Skeleton className="h-7 w-4/5" />
          </li>
        ) : error ? (
          <li className="flex items-center justify-between gap-2 p-2 text-xs text-muted-foreground" role="alert">
            Could not load {kind === "projects" ? "projects" : "tasks"}.
            <Button type="button" variant="outline" size="sm" onClick={retry}>
              Retry
            </Button>
          </li>
        ) : items.length === 0 ? (
          <li className="p-3 text-center text-xs text-muted-foreground" aria-live="polite">
            No {kind === "projects" ? "projects" : "tasks"} match — try a different search.
          </li>
        ) : (
          items.map((item, i) => (
            <li key={item.id} role="none">
              <button
                type="button"
                role="option"
                aria-selected={i === activeIndex}
                onClick={() => void pick(item.id)}
                onMouseEnter={() => setActiveIndex(i)}
                className={cn(
                  "flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm",
                  i === activeIndex && "bg-accent",
                )}
              >
                <span className="truncate">{item.label}</span>
                <span className="flex shrink-0 items-center gap-1.5 text-2xs text-muted-foreground">
                  {busyId === item.id ? <Loader2 className="h-3 w-3 animate-spin" aria-hidden /> : null}
                  {item.hint}
                </span>
              </button>
            </li>
          ))
        )}
      </ul>
    </div>
  );
}
