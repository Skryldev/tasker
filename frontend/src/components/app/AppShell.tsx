import { lazy, Suspense, useEffect, useState, type ReactNode , useRef } from "react";

// Bundle offset (Phase 12.5): the 16-theme picker content is code-split —
// the eager shell keeps only the trigger button + preview wiring.
const ThemeSwitcherContent = lazy(() =>
  import("./ThemeSwitcherContent").then((m) => ({ default: m.ThemeSwitcherContent })),
);
import { NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import {
  ClipboardList,
  FolderKanban,
  Gauge,
  ListTree,
  ListTodo,
  LogOut,
  Menu,
  Palette,
  PanelLeftClose,
  Settings,
  ShieldAlert,
  Target,
} from "lucide-react";
import { NotificationBell } from "@/components/app/NotificationBell";
import { Button } from "@/components/ui/button";
import { api } from "@/api/client";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Avatar, Separator } from "@/components/ui/avatar";
import { useTheme, type ThemeId } from "@/lib/theme";
import { setOpenSettingsHandler } from "@/components/app/shortcut-refs";

// Bundle offset (Phase 13): the team switcher menu is code-split — the
// eager shell keeps the trigger (avatar + name).
const TeamSwitcherMenu = lazy(() =>
  import("./TeamSwitcherMenu").then((m) => ({ default: m.TeamSwitcherMenu })),
);

// Bundle offset (Phase 13 Bug 6): the mobile navigation drawer is
// code-split — radix Dialog leaves the eager shell; it loads on first open.
const MobileNav = lazy(() =>
  import("@/components/app/MobileNav").then((m) => ({ default: m.MobileNav })),
);

// Bundle offset (Phase 12.5): the shortcuts manager (global listener +
// cheatsheet trigger) is code-split; the eager shell renders a placeholder
// button that becomes interactive when the chunk arrives.
const LazyShortcutsManager = lazy(() =>
  import("@/components/app/shortcuts").then((m) => ({ default: m.ShortcutsManager })),
);
function ShortcutsManagerSlot() {
  return (
    <Suspense
      fallback={<span className="inline-block h-8 w-8" aria-hidden />}
    >
      <LazyShortcutsManager />
    </Suspense>
  );
}
import { useAuth, useLogout } from "@/lib/auth";
import { teamLink, WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { teamAvatarUrl } from "@/lib/avatar-version";
import { cn } from "@/lib/utils";

function TeamSwitcher({ onNavigate }: { onNavigate?: () => void }) {
  const { teams, teamId } = useWorkspace();
  const [open, setOpen] = useState(false);
  const [opened, setOpened] = useState(false);
  const active = teams.data?.items.find((t) => t.id === teamId);

  return (
    <DropdownMenu
      open={open}
      onOpenChange={(v) => {
        setOpen(v);
        if (v) setOpened(true);
      }}
    >
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="h-9 w-full justify-start gap-2 px-2" data-testid="team-switcher">
          {teamId ? (
            <Avatar
              name={active?.name ?? "?"}
              size="sm"
              className="h-6 w-6 rounded-md text-2xs"
              src={teamAvatarUrl(teamId)}
            />
          ) : (
            <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-primary/15 text-2xs font-bold text-primary">
              {(active?.name ?? "?").charAt(0).toUpperCase()}
            </span>
          )}
          <span className="truncate text-sm font-medium">{active?.name ?? "Select team"}</span>
        </Button>
      </DropdownMenuTrigger>
      {opened ? (
        <Suspense fallback={null}>
          <TeamSwitcherMenu
            teams={teams.data?.items ?? []}
            activeTeamId={teamId}
            onNavigated={() => {
              setOpen(false);
              onNavigate?.();
            }}
          />
        </Suspense>
      ) : null}
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
  collapsed,
  linkRef,
}: {
  to: string;
  icon: ReactNode;
  label: string;
  end?: boolean;
  /** Treat any route under this prefix (segment boundary) as active. */
  matchPrefix?: string;
  onNavigate?: () => void;
  /** Bug 4: icon-only rail — the label hides; a title tooltip carries it. */
  collapsed?: boolean;
  linkRef?: React.RefObject<HTMLAnchorElement | null>;
}) {
  const { pathname } = useLocation();
  const prefixActive =
    matchPrefix !== undefined &&
    (pathname === matchPrefix || pathname.startsWith(`${matchPrefix}/`));
  return (
    <NavLink
      ref={linkRef as unknown as React.Ref<HTMLAnchorElement>}
      to={to}
      end={end}
      onClick={onNavigate}
      title={collapsed ? label : undefined}
      aria-current={end || matchPrefix ? (prefixActive ? "page" : undefined) : undefined}
      className={({ isActive }) => {
        const active = matchPrefix !== undefined ? prefixActive : isActive;
        return `flex h-9 items-center gap-2.5 rounded-md px-2 text-sm transition-colors duration-100 hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
          collapsed ? "justify-center" : ""
        } ${active ? "bg-primary/10 font-medium text-primary" : ""}`;
      }}
    >
      {icon}
      {collapsed ? null : <span className="truncate">{label}</span>}
    </NavLink>
  );
}

export function SidebarNav({
  onNavigate,
  collapsed,
  firstLinkRef,
}: {
  onNavigate?: () => void;
  collapsed?: boolean;
  firstLinkRef?: React.RefObject<HTMLAnchorElement | null>;
}) {
  const { teamId } = useWorkspace();
  return (
    <>
      <TeamSwitcher onNavigate={onNavigate} />
      <Separator className="my-2" />
      <nav className="flex flex-col gap-0.5" aria-label="Main">
        <SidebarLink linkRef={firstLinkRef} to={teamLink(teamId, "tasks")} icon={<ListTodo className="h-4 w-4" />} label="My Tasks" end onNavigate={onNavigate} collapsed={collapsed} />
        {teamId ? (
          <>
            <SidebarLink
              to={`/projects?team=${teamId}`}
              icon={<FolderKanban className="h-4 w-4" />}
              label="Projects"
              matchPrefix="/projects"
              onNavigate={onNavigate}
              collapsed={collapsed}
            />
            <SidebarLink
              to={`/teams/${teamId}/goals`}
              icon={<Target className="h-4 w-4" />}
              label="Goals"
              matchPrefix={`/teams/${teamId}/goals`}
              onNavigate={onNavigate}
              collapsed={collapsed}
            />
            <SidebarLink
              to={`/teams/${teamId}/dashboard`}
              icon={<Gauge className="h-4 w-4" />}
              label="Dashboard"
              matchPrefix={`/teams/${teamId}/dashboard`}
              onNavigate={onNavigate}
              collapsed={collapsed}
            />
            <SidebarLink
              to={`/teams/${teamId}/workload`}
              icon={<ListTree className="h-4 w-4" />}
              label="Workload"
              matchPrefix={`/teams/${teamId}/workload`}
              onNavigate={onNavigate}
              collapsed={collapsed}
            />
            <SidebarLink
              to={`/teams/${teamId}/capacity`}
              icon={<Gauge className="h-4 w-4" />}
              label="Capacity"
              matchPrefix={`/teams/${teamId}/capacity`}
              onNavigate={onNavigate}
              collapsed={collapsed}
            />
            <SidebarLink
              to={`/teams/${teamId}/settings`}
              icon={<Settings className="h-4 w-4" />}
              label="Team Settings"
              matchPrefix={`/teams/${teamId}/settings`}
              onNavigate={onNavigate}
              collapsed={collapsed}
            />
          </>
        ) : null}
      </nav>
    </>
  );
}

const SIDEBAR_OPEN_KEY = "tasker:sidebar-open";

/** Bug 4: collapsible sidebar. Width transition (200ms ease-out): the rail
    contains flow content (labels + the team switcher), so a transform
    slide would need a second paint surface and clip the focus ring — width
    on a 240px column is a cheap, honest choice. */
function Sidebar() {
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(SIDEBAR_OPEN_KEY) === "collapsed";
    } catch {
      return false;
    }
  });
  const firstLinkRef = useRef<HTMLAnchorElement>(null);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const mobileNav = useState(false);
  void mobileNav;

  const toggle = () => {
    setCollapsed((c) => {
      const next = !c;
      try {
        localStorage.setItem(SIDEBAR_OPEN_KEY, next ? "collapsed" : "open");
      } catch {
        // private mode: in-session state still works
      }
      // Focus management: collapsing → toggle; expanding → first nav item.
      requestAnimationFrame(() => {
        if (next) toggleRef.current?.focus();
        else firstLinkRef.current?.focus();
      });
      return next;
    });
  };

  // ⌘/Ctrl+B toggles (global; works while typing — a layout command).
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "b") {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  return (
    <aside
      className={cn(
        "flex h-full shrink-0 flex-col gap-1 overflow-hidden border-r bg-sidebar p-3 text-sidebar-foreground transition-[width] duration-200 ease-out",
        collapsed ? "w-12" : "w-60",
      )}
      data-testid="app-sidebar"
      aria-label="Main navigation"
    >
      <button
        type="button"
        ref={toggleRef}
        onClick={toggle}
        aria-expanded={!collapsed}
        aria-label={collapsed ? "Expand navigation sidebar" : "Collapse navigation sidebar"}
        className="mb-1 flex h-8 w-full items-center justify-center gap-2 rounded-md text-sidebar-foreground/70 transition-colors hover:bg-accent hover:text-sidebar-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        data-testid="sidebar-toggle"
      >
        <PanelLeftClose className={cn("h-4 w-4 shrink-0 transition-transform duration-200", collapsed && "rotate-180")} />
        {collapsed ? null : <span className="text-xs font-medium">Collapse</span>}
      </button>
      <div className={cn("flex min-h-0 flex-1 flex-col", collapsed && "items-center")}>
        <SidebarNav collapsed={collapsed} firstLinkRef={firstLinkRef} />
      </div>
      {collapsed ? null : (
        <div className="pb-1 pl-2 text-2xs text-muted-foreground">Tasker 0.1</div>
      )}
    </aside>
  );
}


function ThemeSwitcher() {
  const { theme, setTheme } = useTheme();
  const [open, setOpen] = useState(false);
  const [opened, setOpened] = useState(false);
  // Hover preview: temporarily apply the hovered theme's tokens on <html>
  // without touching provider state or localStorage. Closing the menu (or
  // leaving an item) restores the committed theme.
  const [preview, setPreview] = useState<ThemeId | null>(null);

  useEffect(() => {
    document.documentElement.setAttribute(
      "data-theme",
      preview ?? theme,
    );
    return () => document.documentElement.removeAttribute("data-theme");
  }, [preview, theme]);

  return (
    <DropdownMenu
      open={open}
      onOpenChange={(v) => {
        setOpen(v);
        if (v) setOpened(true);
        if (!v) setPreview(null);
      }}
    >
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="Change theme" data-testid="theme-switcher">
          <Palette />
        </Button>
      </DropdownMenuTrigger>
      {opened ? (
        <Suspense fallback={null}>
          <ThemeSwitcherContent
            theme={theme}
            onPreview={setPreview}
            onSelect={(id) => {
              setTheme(id);
              setOpen(false);
            }}
          />
        </Suspense>
      ) : null}
    </DropdownMenu>
  );
}

/** Phase 13 (Bug 7): reminds the user they are acting as a team guest —
    restricted, explicitly granted access. Only when the active context is
    a guest membership. */
function GuestBadge() {
  const { myRole } = useWorkspace();
  if (myRole !== "guest") return <NotificationBell />;
  return (
    <>
      <span
        className="inline-flex items-center gap-1 rounded-full border border-warning/50 bg-warning/10 px-2 py-0.5 text-2xs font-medium text-warning"
        title="You are a guest of this team — access is limited to what the Owner granted."
        data-testid="guest-badge"
      >
        <ShieldAlert className="h-3 w-3" aria-hidden /> Guest
      </span>
      <NotificationBell />
    </>
  );
}

function Topbar() {
  const { user } = useAuth();
  const logout = useLogout();
  const navigate = useNavigate();
  const [navOpen, setNavOpen] = useState(false);
  // Bug 5: publish the Settings opener for the lazy shortcuts manager
  // (⌘/Ctrl+,).
  useEffect(() => {
    setOpenSettingsHandler(() => navigate("/settings"));
    return () => setOpenSettingsHandler(null);
  }, [navigate]);

  return (
    <header className="flex h-12 shrink-0 items-center justify-between border-b px-4">
      <Suspense fallback={null}>
        <MobileNav open={navOpen} onOpenChange={setNavOpen} />
      </Suspense>
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
        <GuestBadge />
        <ShortcutsManagerSlot />
        <ThemeSwitcher />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" aria-label="Account menu" data-testid="user-menu">
              <Avatar
                name={user?.display_name ?? "?"}
                src={user ? api.userAvatarUrl(user.id) : undefined}
              />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-52">
            <DropdownMenuLabel>
              <div className="text-sm font-medium text-foreground">{user?.display_name}</div>
              <div className="truncate text-2xs font-normal">{user?.email}</div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => navigate("/settings")} data-testid="menu-settings">
              <Settings /> Settings
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
