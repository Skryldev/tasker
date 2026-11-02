import { Suspense, useState, type ReactNode } from "react";
import { NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import {
  ClipboardList,
  FolderKanban,
  ListTree,
  ListTodo,
  LogOut,
  Menu,
  Moon,
  Settings,
  Sun,
  Target,
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
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Avatar, Separator } from "@/components/ui/avatar";
import { useTheme } from "@/lib/theme";
import { useAuth, useLogout } from "@/lib/auth";
import { WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { roleLabel } from "@/lib/utils";

function TeamSwitcher({ onNavigate }: { onNavigate?: () => void }) {
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
            onSelect={() => {
              navigate(`/?team=${team.id}`);
              onNavigate?.();
            }}
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
        <DropdownMenuItem
          onSelect={() => {
            navigate("/teams/new");
            onNavigate?.();
          }}
        >
          Create team…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function SidebarLink({
  to,
  icon,
  label,
  end,
  matchPrefix,
  onNavigate,
}: {
  to: string;
  icon: ReactNode;
  label: string;
  end?: boolean;
  /** Treat any route under this prefix (segment boundary) as active. */
  matchPrefix?: string;
  onNavigate?: () => void;
}) {
  const { pathname } = useLocation();
  const prefixActive =
    matchPrefix !== undefined &&
    (pathname === matchPrefix || pathname.startsWith(`${matchPrefix}/`));
  return (
    <NavLink
      to={to}
      end={end}
      onClick={onNavigate}
      aria-current={end || matchPrefix ? (prefixActive ? "page" : undefined) : undefined}
      className={({ isActive }) => {
        const active = matchPrefix !== undefined ? prefixActive : isActive;
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

function SidebarNav({ onNavigate }: { onNavigate?: () => void }) {
  const { teamId } = useWorkspace();
  return (
    <>
      <TeamSwitcher onNavigate={onNavigate} />
      <Separator className="my-2" />
      <nav className="flex flex-col gap-0.5" aria-label="Main">
        <SidebarLink to="/" icon={<ListTodo />} label="My Tasks" end onNavigate={onNavigate} />
        {teamId ? (
          <>
            <SidebarLink
              to={`/projects?team=${teamId}`}
              icon={<FolderKanban />}
              label="Projects"
              matchPrefix="/projects"
              onNavigate={onNavigate}
            />
            <SidebarLink
              to={`/teams/${teamId}/goals`}
              icon={<Target />}
              label="Goals"
              matchPrefix={`/teams/${teamId}/goals`}
              onNavigate={onNavigate}
            />
            <SidebarLink
              to={`/teams/${teamId}/workload`}
              icon={<ListTree />}
              label="Workload"
              matchPrefix={`/teams/${teamId}/workload`}
              onNavigate={onNavigate}
            />
            <SidebarLink
              to={`/teams/${teamId}/settings`}
              icon={<Settings />}
              label="Team Settings"
              matchPrefix={`/teams/${teamId}/settings`}
              onNavigate={onNavigate}
            />
          </>
        ) : null}
      </nav>
    </>
  );
}

function Sidebar() {
  return (
    <aside className="flex h-full w-60 shrink-0 flex-col gap-1 border-r bg-sidebar p-3 text-sidebar-foreground">
      <SidebarNav />
      <div className="mt-auto pb-1 pl-2 text-2xs text-muted-foreground">Tasker 0.1</div>
    </aside>
  );
}

/** Slide-in navigation for widths below `sm`, where the sidebar is hidden. */
function MobileNav({ open, onOpenChange }: { open: boolean; onOpenChange: (v: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="left-0 top-0 h-full max-h-none w-72 max-w-[85vw] translate-x-0 translate-y-0 rounded-none rounded-r-lg border-r p-3 data-[state=open]:slide-in-from-left data-[state=closed]:slide-out-to-left"
        aria-describedby={undefined}
      >
        <DialogTitle className="text-sm font-semibold">Navigation</DialogTitle>
        <nav className="mt-3 flex flex-col" aria-label="Mobile">
          <SidebarNav onNavigate={() => onOpenChange(false)} />
        </nav>
        <div className="mt-auto pb-1 pl-2 text-2xs text-muted-foreground">Tasker 0.1</div>
      </DialogContent>
    </Dialog>
  );
}

function Topbar() {
  const { user } = useAuth();
  const logout = useLogout();
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();
  const [navOpen, setNavOpen] = useState(false);

  return (
    <header className="flex h-12 shrink-0 items-center justify-between border-b px-4">
      <MobileNav open={navOpen} onOpenChange={setNavOpen} />
      <div className="flex items-center gap-2 text-sm font-semibold">
        <Button
          variant="ghost"
          size="icon-sm"
          className="sm:hidden"
          aria-label="Open navigation"
          aria-expanded={navOpen}
          onClick={() => setNavOpen(true)}
          data-testid="mobile-nav"
        >
          <Menu />
        </Button>
        <ClipboardList className="h-4 w-4 text-primary" aria-hidden />
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

/** Fallback while a lazily imported route chunk loads. */
function RouteFallback() {
  return (
    <div className="flex h-full min-h-40 items-center justify-center" aria-busy="true" role="status">
      <span
        className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent"
        aria-label="Loading"
      />
    </div>
  );
}

function AppShellInner() {
  return (
    <div className="flex h-screen flex-col">
      {/* Skip link: first focusable element, visible only when focused. */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:z-[60] focus:rounded-md focus:bg-primary focus:px-3 focus:py-1.5 focus:text-sm focus:text-primary-foreground"
      >
        Skip to main content
      </a>
      <Topbar />
      <div className="flex min-h-0 flex-1">
        <div className="hidden sm:flex">
          <Sidebar />
        </div>
        <main
          id="main-content"
          className="min-w-0 flex-1 overflow-y-auto scrollbar-thin"
          data-testid="main-content"
        >
          {/* One boundary covers every lazily imported route below. */}
          <Suspense fallback={<RouteFallback />}>
            <Outlet />
          </Suspense>
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
