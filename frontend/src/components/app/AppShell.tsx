import { useState, type ReactNode } from "react";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import {
  ClipboardList,
  FolderKanban,
  ListTodo,
  LogOut,
  Moon,
  Settings,
  Sun,
  UserRound,
} from "lucide-react";
import { NotificationBell } from "@/components/app/NotificationBell";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Avatar, Separator } from "@/components/ui/avatar";
import { useTheme } from "@/lib/theme";
import { useAuth, useLogout } from "@/lib/auth";
import { WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { roleLabel } from "@/lib/utils";

function TeamSwitcher() {
  const { teams, teamId } = useWorkspace();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const active = teams.data?.items.find((t) => t.id === teamId);

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="h-9 w-full justify-start gap-2 px-2" data-testid="team-switcher">
          <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-primary/15 text-2xs font-bold text-primary">
            {(active?.name ?? "?").charAt(0).toUpperCase()}
          </span>
          <span className="truncate text-sm font-medium">{active?.name ?? "Select team"}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        <DropdownMenuLabel>Teams</DropdownMenuLabel>
        {teams.data?.items.map((team) => (
          <DropdownMenuItem
            key={team.id}
            onSelect={() => navigate(`/?team=${team.id}`)}
            className="justify-between"
          >
            <span className="truncate">{team.name}</span>
            <span className="text-2xs text-muted-foreground">{roleLabel(team.my_role ?? "")}</span>
          </DropdownMenuItem>
        ))}
        {teams.data?.items.length === 0 ? (
          <DropdownMenuItem disabled>No teams yet</DropdownMenuItem>
        ) : null}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => navigate("/teams/new")}>Create team…</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Sidebar() {
  const { teamId } = useWorkspace();
  return (
    <aside className="flex h-full w-60 shrink-0 flex-col gap-1 border-r bg-sidebar p-3 text-sidebar-foreground">
      <TeamSwitcher />
      <Separator className="my-2" />
      <nav className="flex flex-col gap-0.5" aria-label="Main">
        <SidebarLink to="/" icon={<ListTodo />} label="My Tasks" end />
        {teamId ? (
          <>
            <SidebarLink
              to={`/projects?team=${teamId}`}
              icon={<FolderKanban />}
              label="Projects"
              matchPrefix="/projects"
            />
            <SidebarLink
              to={`/teams/${teamId}/settings`}
              icon={<Settings />}
              label="Team Settings"
              matchPrefix="/teams"
            />
          </>
        ) : null}
      </nav>
      <div className="mt-auto pb-1 pl-2 text-2xs text-muted-foreground">Tasker 0.1</div>
    </aside>
  );
}

function SidebarLink({
  to,
  icon,
  label,
  end,
  matchPrefix,
}: {
  to: string;
  icon: ReactNode;
  label: string;
  end?: boolean;
  matchPrefix?: string;
}) {
  return (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) => {
        const active = matchPrefix ? location.pathname.startsWith(matchPrefix) : isActive;
        return `flex items-center gap-2.5 rounded-md px-2 py-1.5 text-sm transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
          active ? "bg-accent font-medium text-accent-foreground" : ""
        }`;
      }}
    >
      {icon}
      {label}
    </NavLink>
  );
}

function Topbar() {
  const { user } = useAuth();
  const logout = useLogout();
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();

  return (
    <header className="flex h-12 shrink-0 items-center justify-between border-b px-4">
      <div className="flex items-center gap-2 text-sm font-semibold">
        <ClipboardList className="h-4 w-4 text-primary" />
        Tasker
      </div>
      <div className="flex items-center gap-1.5">
        <NotificationBell />
        <Button
          variant="ghost"
          size="icon"
          onClick={toggle}
          aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          data-testid="theme-toggle"
        >
          {theme === "dark" ? <Sun /> : <Moon />}
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" aria-label="Account menu" data-testid="user-menu">
              <Avatar name={user?.display_name ?? "?"} />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-52">
            <DropdownMenuLabel>
              <div className="text-sm font-medium text-foreground">{user?.display_name}</div>
              <div className="truncate text-2xs font-normal">{user?.email}</div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => navigate("/account")}>
              <UserRound /> Account
            </DropdownMenuItem>
            <DropdownMenuItem destructive onSelect={() => logout.mutate()} data-testid="logout">
              <LogOut /> Log out
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </header>
  );
}

function AppShellInner() {
  return (
    <div className="flex h-screen flex-col">
      <Topbar />
      <div className="flex min-h-0 flex-1">
        <div className="hidden sm:flex">
          <Sidebar />
        </div>
        <main className="min-w-0 flex-1 overflow-y-auto scrollbar-thin" data-testid="main-content">
          <Outlet />
        </main>
      </div>
    </div>
  );
}

export function AppShell() {
  return (
    <WorkspaceProvider>
      <AppShellInner />
    </WorkspaceProvider>
  );
}
