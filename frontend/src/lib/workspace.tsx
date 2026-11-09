import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useTeams } from "@/api/hooks";

/**
 * Bug 3 (Phase 12.5): the URL is the single source of truth for the active
 * team. `:teamId` path segments win (team-scoped sections), then the
 * `?team=` query param (list surfaces). A missing team defaults ONCE from
 * the persisted last team (localStorage, next-visit convenience only) or
 * the first team — never overriding a URL that already names a team.
 */

const LAST_TEAM_KEY = "tasker.lastTeam";

export function readLastTeamId(): string | null {
  try {
    return localStorage.getItem(LAST_TEAM_KEY);
  } catch {
    return null;
  }
}

function writeLastTeamId(teamId: string): void {
  try {
    localStorage.setItem(LAST_TEAM_KEY, teamId);
  } catch {
    // Private-mode: defaulting just falls back to the first team.
  }
}

/**
 * Pure link/switch helpers (unit-tested): every team-scoped link carries the
 * URL's team id, and switching teams preserves the current section.
 */

/** Sidebar links: `/` (My Tasks) and `/projects` carry `?team=...`. */
export function teamLink(teamId: string | undefined, section: "tasks" | "projects"): string {
  const base = section === "tasks" ? "/" : "/projects";
  return teamId ? `${base}?team=${teamId}` : base;
}

/**
 * Switch to `newTeamId` preserving the current section: rewrite the
 * `:teamId` path segment on team-scoped routes, else replace `?team=`.
 *
 * Phase 13 (Bug 2): a PROJECT DETAIL path must never be preserved — a
 * project belongs to exactly one team, so carrying
 * `/projects/:projectId` into the new team rendered the OLD team's
 * project (with its tasks) under the new team's `?team=` — reading as a
 * cross-team data leak. The switch lands on the new team's projects list
 * instead.
 */
export function switchTeamLocation(
  pathname: string,
  search: string,
  newTeamId: string,
): { pathname: string; search: string } {
  const pathMatch = pathname.match(/^\/teams\/([^/]+)(\/.*)?$/);
  if (pathMatch) {
    const rest = pathMatch[2] ?? "/";
    return { pathname: `/teams/${newTeamId}${rest}`, search };
  }
  // Project detail (and any nested view of one) belongs to the old team.
  if (pathname.startsWith("/projects/")) {
    return { pathname: "/projects", search: `team=${newTeamId}` };
  }
  const next = new URLSearchParams(search);
  next.set("team", newTeamId);
  return { pathname, search: next.toString() };
}

interface WorkspaceContextValue {
  /** Active team id — derived from the URL (path segment, then `?team=`). */
  teamId: string | undefined;
  teamName: string | undefined;
  myRole: string | undefined;
  teams: ReturnType<typeof useTeams>;
}

const WorkspaceContext = createContext<WorkspaceContextValue>({
  teamId: undefined,
  teamName: undefined,
  myRole: undefined,
  teams: { data: undefined, isLoading: false } as ReturnType<typeof useTeams>,
});

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const teams = useTeams();
  const params = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  // `:teamId` path segment (routes: teams/:teamId/...) wins over `?team=`.
  const pathTeam = (params as Record<string, string | undefined>).teamId;
  const paramTeam = searchParams.get("team") ?? undefined;
  const requested = pathTeam ?? paramTeam;

  // Defaulting runs ONLY when the URL names no team at all, and only to
  // fill the `?team=` param for list surfaces: the persisted last team when
  // it still exists, else the first team. A URL that names a team is never
  // overridden — it is the authority (Bug 3).
  useEffect(() => {
    if (window.location.pathname === "/teams/new") return;
    if (requested) {
      writeLastTeamId(requested);
      return;
    }
    if (!teams.data) return;
    const stored = readLastTeamId();
    const candidate =
      teams.data.items.find((t) => t.id === stored) ?? teams.data.items[0];
    if (candidate) {
      const next = new URLSearchParams(searchParams);
      next.set("team", candidate.id);
      setSearchParams(next, { replace: true });
    }
  }, [requested, teams.data, searchParams, setSearchParams]);

  const active = useMemo(
    () => teams.data?.items.find((t) => t.id === requested),
    [teams.data, requested],
  );

  const value = useMemo(
    () => ({
      teamId: active?.id,
      teamName: active?.name,
      myRole: active?.my_role ?? undefined,
      teams,
    }),
    [active, teams],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace(): WorkspaceContextValue {
  return useContext(WorkspaceContext);
}

/** The caller's role in the active team ("" when unknown). */
export function useMyRole(): string {
  return useContext(WorkspaceContext).myRole ?? "";
}

export function useCanManageProjects(): boolean {
  const role = useMyRole();
  return ["owner", "admin", "manager"].includes(role);
}

export function useIsAdmin(): boolean {
  const role = useMyRole();
  return ["owner", "admin"].includes(role);
}
