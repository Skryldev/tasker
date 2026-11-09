import { useNavigate, useLocation } from "react-router-dom";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { roleLabel } from "@/lib/utils";
import { switchTeamLocation } from "@/lib/workspace";

/**
 * Team switcher menu content (Phase 13 bundle offset): code-split from the
 * eager shell like the theme picker — the trigger renders the avatar +
 * name; the menu (team list, switch navigation, create-team) loads on
 * first open.
 */
export function TeamSwitcherMenu({
  teams,
  activeTeamId,
  onNavigated,
}: {
  teams: { id: string; name: string; my_role?: string | null }[];
  activeTeamId: string | undefined;
  onNavigated?: () => void;
}) {
  const navigate = useNavigate();
  const location = useLocation();
  return (
    <DropdownMenuContent align="start" className="w-56">
      <DropdownMenuLabel>Teams</DropdownMenuLabel>
      {teams.map((team) => (
        <DropdownMenuItem
          key={team.id}
          onSelect={() => {
            // Bug 3: preserve the current section — rewrite the `:teamId`
            // path segment on team-scoped routes; project paths land on the
            // new team's projects list (ADR-0017).
            const next = switchTeamLocation(location.pathname, location.search, team.id);
            navigate(next);
            onNavigated?.();
          }}
          className="justify-between"
        >
          <span className="truncate">{team.name}</span>
          <span className="text-2xs text-muted-foreground">{roleLabel(team.my_role ?? "")}</span>
        </DropdownMenuItem>
      ))}
      {teams.length === 0 ? (
        <DropdownMenuItem disabled>No teams yet</DropdownMenuItem>
      ) : null}
      <DropdownMenuSeparator />
      <DropdownMenuItem
        onSelect={() => {
          navigate("/teams/new");
          onNavigated?.();
        }}
      >
        Create team…
      </DropdownMenuItem>
      {activeTeamId ? null : null}
    </DropdownMenuContent>
  );
}
