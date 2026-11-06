import { StrictMode, Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import "@/index.css";
import { QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, Navigate, RouterProvider } from "react-router-dom";
import { AppShell } from "@/components/app/AppShell";
import { ThemeProvider } from "@/lib/theme";
import { ToastProvider } from "@/components/ui/toast";
import { AuthProvider, handleUnauthorizedError, useAuth } from "@/lib/auth";
import { MyTasksPage } from "@/pages/MyTasksPage";
import type { ReactNode } from "react";

// Route-level code splitting: only the shell and the default landing page
// load eagerly. The login page is lazy too — its form libraries (react-hook
// form + zod) are irrelevant to the authenticated app, which is the route
// users live in (see vite.config manualChunks).
const LoginPage = lazy(() =>
  import("@/pages/LoginPage").then((m) => ({ default: m.LoginPage })),
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
const AccountPage = lazy(() =>
  import("@/pages/AccountPage").then((m) => ({ default: m.AccountPage })),
);
const TaskPage = lazy(() => import("@/pages/TaskPage").then((m) => ({ default: m.TaskPage })));
const WorkloadPage = lazy(() =>
  import("@/pages/WorkloadPage").then((m) => ({ default: m.WorkloadPage })),
);
const CapacityPage = lazy(() =>
  import("@/pages/CapacityPage").then((m) => ({ default: m.CapacityPage })),
);
const GoalsPage = lazy(() => import("@/pages/GoalsPage").then((m) => ({ default: m.GoalsPage })));
const GoalDetailPage = lazy(() =>
  import("@/pages/GoalDetailPage").then((m) => ({ default: m.GoalDetailPage })),
);
const ProjectGuestsPage = lazy(() =>
  import("@/pages/ProjectGuestsPage").then((m) => ({ default: m.ProjectGuestsPage })),
);

const router = createBrowserRouter([
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
      { index: true, element: <MyTasksPage /> },
      { path: "projects", element: <ProjectsPage /> },
      { path: "projects/:projectId", element: <ProjectPage /> },
      { path: "teams/new", element: <CreateTeamPage /> },
      { path: "teams/:teamId/workload", element: <WorkloadPage /> },
      { path: "teams/:teamId/capacity", element: <CapacityPage /> },
      { path: "teams/:teamId/goals", element: <GoalsPage /> },
      { path: "goals/:goalId", element: <GoalDetailPage /> },
      { path: "teams/:teamId/settings", element: <TeamSettingsPage /> },
      { path: "projects/:projectId/guests", element: <ProjectGuestsPage /> },
      { path: "tasks/:taskId", element: <TaskPage /> },
      { path: "account", element: <AccountPage /> },
      { path: "*", element: <Navigate to="/" replace /> },
    ],
  },
]);

const queryClient = new QueryClient({
  queryCache: new QueryCache({
    // A 401 from any query means the session is gone: clear all cached
    // server state and redirect. Non-401 errors keep their in-page error UI.
    onError: (error) =>
      handleUnauthorizedError(error, queryClient, (path) => {
        void router.navigate(path, { replace: true });
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

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <ToastProvider>
          <AuthProvider>
            <RouterProvider router={router} />
          </AuthProvider>
        </ToastProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </StrictMode>,
);
