import { StrictMode, lazy } from "react";
import { createRoot } from "react-dom/client";
import "@/index.css";
import { QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, Navigate, RouterProvider } from "react-router-dom";
import { AppShell } from "@/components/app/AppShell";
import { ThemeProvider } from "@/lib/theme";
import { ToastProvider } from "@/components/ui/toast";
import { AuthProvider, handleUnauthorizedError, useAuth } from "@/lib/auth";
import { LoginPage } from "@/pages/LoginPage";
import { MyTasksPage } from "@/pages/MyTasksPage";
import type { ReactNode } from "react";

// Route-level code splitting: the login page and the default landing page
// load eagerly with the shell; every other page is fetched on first visit so
// the initial bundle stays small (see vite.config manualChunks).
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
const GoalsPage = lazy(() => import("@/pages/GoalsPage").then((m) => ({ default: m.GoalsPage })));
const GoalDetailPage = lazy(() =>
  import("@/pages/GoalDetailPage").then((m) => ({ default: m.GoalDetailPage })),
);
const ProjectGuestsPage = lazy(() =>
  import("@/pages/ProjectGuestsPage").then((m) => ({ default: m.ProjectGuestsPage })),
);

const router = createBrowserRouter([
  { path: "/login", element: <LoginPage /> },
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

/** Redirects unauthenticated visitors to the login screen. */
function RequireAuth({ children }: { children: ReactNode }) {
  const { user, isLoading } = useAuth();
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
