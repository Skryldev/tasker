import { useNavigate, useLocation } from "react-router-dom";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { useTranslation } from "react-i18next";
import { roleLabelKey } from "@/lib/utils";
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
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  return (
    // Long team lists scroll instead of clipping the create-team action
    // below the fold (the menu itself is overflow-hidden by default).
    <DropdownMenuContent
      align="start"
      className="w-56 max-h-[min(24rem,60vh)] overflow-y-auto scrollbar-thin"
    >
      <DropdownMenuLabel>{t("team.label")}</DropdownMenuLabel>
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
          <span className="text-2xs text-muted-foreground">{t(roleLabelKey(team.my_role ?? ""))}</span>
        </DropdownMenuItem>
      ))}
      {teams.length === 0 ? (
        <DropdownMenuItem disabled>{t("team.noneYet")}</DropdownMenuItem>
      ) : null}
      <DropdownMenuSeparator />
      <DropdownMenuItem
        onSelect={() => {
          navigate("/teams/new");
          onNavigated?.();
        }}
      >
        {t("team.create")}
      </DropdownMenuItem>
      {activeTeamId ? null : null}
    </DropdownMenuContent>
  );
}
