import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import { useTeams } from "@/api/hooks";

interface WorkspaceContextValue {
  /** Active team id (reflected in the ?team= query param). */
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
  const [params, setParams] = useSearchParams();
  const requested = params.get("team") ?? undefined;

  // Default to the first team once loaded; keep the URL shareable. Skipped
  // on the create-team page: there is no active workspace there, and
  // defaulting the param would leak the previous team into the new team's
  // follow-up navigation.
  useEffect(() => {
    if (window.location.pathname === "/teams/new") return;
    if (!requested && teams.data) {
      const first = teams.data.items[0];
      if (first) {
        const next = new URLSearchParams(params);
        next.set("team", first.id);
        setParams(next, { replace: true });
      }
    }
  }, [requested, teams.data, params, setParams]);

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
