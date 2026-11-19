import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import "@/index.css";
import { QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, Navigate, RouterProvider } from "react-router-dom";
import { DirectionProvider } from "@radix-ui/react-direction";
import { AppShell } from "@/components/app/AppShell";
import { ThemeProvider } from "@/lib/theme";
import { ToastProvider } from "@/components/ui/toast";
import { AuthProvider, handleUnauthorizedError, useAuth } from "@/lib/auth";
import type { ReactNode } from "react";
import {
  LOCALE_META,
  applyDocumentLocale,
  localeFromPath,
  resolveInitialLocale,
  stripLocalePrefix,
  type Locale,
} from "@/i18n/locale";
import { initI18n } from "@/i18n/config";

// Route-level code splitting: every route is lazy, including login and the
// default landing page (the shell covers first paint). — its form libraries (react-hook
// form + zod) are irrelevant to the authenticated app, which is the route
// users live in (see vite.config manualChunks).
const LoginPage = lazy(() =>
  import("@/pages/LoginPage").then((m) => ({ default: m.LoginPage })),
);
const MyTasksPage = lazy(() =>
  import("@/pages/MyTasksPage").then((m) => ({ default: m.MyTasksPage })),
);
const ProjectsPage = lazy(() =>
  import("@/pages/ProjectsPage").then((m) => ({ default: m.ProjectsPage })),
);
const ProjectPage = lazy(() =>
  import("@/pages/ProjectPage").then((m) => ({ default: m.ProjectPage })),
);
const TeamSettingsPage = lazy(() =>
  import("@/pages/TeamSettingsPage").then((m) => ({ default: m.TeamSettingsPage })),
);
const CreateTeamPage = lazy(() =>
  import("@/pages/CreateTeamPage").then((m) => ({ default: m.CreateTeamPage })),
);
const TaskPage = lazy(() => import("@/pages/TaskPage").then((m) => ({ default: m.TaskPage })));
const WorkloadPage = lazy(() =>
  import("@/pages/WorkloadPage").then((m) => ({ default: m.WorkloadPage })),
);
const CapacityPage = lazy(() =>
  import("@/pages/CapacityPage").then((m) => ({ default: m.CapacityPage })),
);
const DashboardPage = lazy(() =>
  import("@/pages/DashboardPage").then((m) => ({ default: m.default })),
);
const SettingsPage = lazy(() =>
  import("@/pages/SettingsPage").then((m) => ({ default: m.default })),
);
const GoalsPage = lazy(() => import("@/pages/GoalsPage").then((m) => ({ default: m.GoalsPage })));
const GoalDetailPage = lazy(() =>
  import("@/pages/GoalDetailPage").then((m) => ({ default: m.GoalDetailPage })),
);
const ProjectGuestsPage = lazy(() =>
  import("@/pages/ProjectGuestsPage").then((m) => ({ default: m.ProjectGuestsPage })),
);

/** Route table — mounted under the locale basename by LocaleRouter. */
function routeTree() {
  return [
    {
      path: "/login",
      element: (
        <Suspense fallback={<LoginFallback />}>
          <LoginPage />
        </Suspense>
      ),
    },
    {
      path: "/",
      element: (
        <RequireAuth>
          <AppShell />
        </RequireAuth>
      ),
      children: [
        {
          index: true,
          element: (
            <Suspense fallback={<RouteFallback />}>
              <MyTasksPage />
            </Suspense>
          ),
        },
        { path: "projects", element: <ProjectsPage /> },
        { path: "projects/:projectId", element: <ProjectPage /> },
        { path: "teams/:teamId/projects/:projectId", element: <ProjectPage /> },
        { path: "teams/:teamId/projects/:projectId/list", element: <ProjectPage /> },
        { path: "teams/:teamId/projects/:projectId/board", element: <ProjectPage /> },
        { path: "teams/:teamId/projects/:projectId/timeline", element: <ProjectPage /> },
        { path: "projects/:projectId/list", element: <ProjectPage /> },
        { path: "projects/:projectId/board", element: <ProjectPage /> },
        { path: "projects/:projectId/timeline", element: <ProjectPage /> },
        { path: "teams/new", element: <CreateTeamPage /> },
        { path: "teams/:teamId/workload", element: <WorkloadPage /> },
        { path: "teams/:teamId/capacity", element: <CapacityPage /> },
        { path: "teams/:teamId/dashboard", element: <DashboardPage /> },
        { path: "teams/:teamId/goals", element: <GoalsPage /> },
        { path: "goals/:goalId", element: <GoalDetailPage /> },
        { path: "teams/:teamId/settings", element: <TeamSettingsPage /> },
        { path: "projects/:projectId/guests", element: <ProjectGuestsPage /> },
        {
          path: "teams/:teamId/projects/:projectId/tasks/:taskId",
          element: <TaskPage />,
        },
        { path: "tasks/:taskId", element: <TaskPage /> },
        { path: "users", element: <Navigate to="/settings#users" replace /> },
        { path: "account", element: <Navigate to="/settings#account" replace /> },
        { path: "settings", element: <SettingsPage /> },
        { path: "*", element: <Navigate to="/" replace /> },
      ],
    },
  ];
}

const queryClient = new QueryClient({
  queryCache: new QueryCache({
    // A 401 from any query means the session is gone: clear all cached
    // server state and redirect. Non-401 errors keep their in-page error UI.
    onError: (error) =>
      handleUnauthorizedError(error, queryClient, (path) => {
        void routerRef.current?.navigate(path, { replace: true });
      }),
  }),
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
      staleTime: 15_000,
    },
  },
});

/** Shared route fallback while any lazy page chunk loads. */
function RouteFallback() {
  return (
    <div className="flex h-screen items-center justify-center" aria-busy="true">
      <span
        className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent"
        aria-label="Loading"
      />
    </div>
  );
}

/** Login-shaped fallback while the lazy login chunk loads (rare: one hop). */
function LoginFallback() {
  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <span
        className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent"
        aria-label="Loading"
      />
    </div>
  );
}

/** Redirects unauthenticated visitors to the login screen. */
function RequireAuth({ children }: { children: ReactNode }) {  const { user, isLoading } = useAuth();
  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center" aria-busy="true">
        <span className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" aria-label="Loading" />
      </div>
    );
  }
  if (!user) {
    return <Navigate to="/login" replace />;
  }
  return <>{children}</>;
}

/**
 * Locale routing (Phase 15, ADR-0024): the locale is the first URL segment.
 * The router carries `basename = /{locale}` so every existing navigate/Link
 * keeps working unchanged; switching locales performs a full-document
 * navigation (rare action) so the basename is rebuilt cleanly.
 */
const routerRef: { current: ReturnType<typeof createBrowserRouter> | null } = { current: null };

function LocaleRouter({ locale }: { locale: Locale }) {
  const router = createBrowserRouter(routeTree(), { basename: `/${locale}` });
  routerRef.current = router;
  return <RouterProvider router={router} />;
}

// Bootstrap: resolve the locale from the URL (or detect + rewrite the
// address), sync <html lang/dir>, init i18n with the eager namespace, then
// mount. The index.html inline script has already applied the same locale
// to the document (font preload + dir) before this module executes.
const initialLocale = resolveInitialLocale();
if (localeFromPath(window.location.pathname) === null) {
  // First visit (or a stale bookmark): make the URL the source of truth.
  const path = stripLocalePrefix(window.location.pathname);
  const search = window.location.search;
  const hash = window.location.hash;
  window.history.replaceState(
    null,
    "",
    `/${initialLocale}${path === "/" ? "/" : path}${search}${hash}`,
  );
}
applyDocumentLocale(initialLocale);

await initI18n(initialLocale);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <DirectionProvider dir={LOCALE_META[initialLocale].dir}>
        <ThemeProvider>
          <ToastProvider>
            <AuthProvider>
              <LocaleRouter locale={initialLocale} />
            </AuthProvider>
          </ToastProvider>
        </ThemeProvider>
      </DirectionProvider>
    </QueryClientProvider>
  </StrictMode>,
);
