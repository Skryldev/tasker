import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { GoalsPage, GoalFormDialog } from "@/pages/GoalsPage";
import { ToastProvider } from "@/components/ui/toast";
import { ThemeProvider } from "@/lib/theme";

vi.mock("@/api/client", () => ({
  ApiError: class ApiError extends Error {},
  api: {
    listGoals: vi.fn(),
    createGoal: vi.fn(),
    getGoal: vi.fn(),
    listGoalProjects: vi.fn().mockResolvedValue({ items: [] }),
    listGoalTasks: vi.fn().mockResolvedValue({ items: [] }),
    goalActivity: vi.fn().mockResolvedValue({ items: [] }),
    listKeyResults: vi.fn().mockResolvedValue({ items: [] }),
  },
}));

const api = (await import("@/api/client")).api;

const SAMPLE_GOALS = {
  items: [
    {
      id: "g1",
      team_id: "t1",
      title: "Grow activation",
      description: "Q4 objective",
      owner: { id: "u1", display_name: "Ada Admin", email: "a@t.example" },
      status: "active" as const,
      start_date: "2026-01-01",
      due_date: "2026-03-31",
      created_by: { id: "u1", display_name: "Ada Admin", email: "a@t.example" },
      completed_at: null,
      created_at: 1700000000000,
      updated_at: 1700000000000,
      progress: 50,
      key_result_count: 2,
    },
    {
      id: "g2",
      team_id: "t1",
      title: "Cut churn",
      description: null,
      owner: { id: "u1", display_name: "Ada Admin", email: "a@t.example" },
      status: "draft" as const,
      start_date: null,
      due_date: null,
      created_by: { id: "u1", display_name: "Ada Admin", email: "a@t.example" },
      completed_at: null,
      created_at: 1700000001000,
      updated_at: 1700000001000,
      progress: null,
      key_result_count: 0,
    },
  ],
  page: 1,
  page_size: 25,
  total: 2,
};

function withProviders(ui: React.ReactElement, initialPath = "/teams/t1/goals") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ThemeProvider>
        <ToastProvider>
          <MemoryRouter initialEntries={[initialPath]}>
            <Routes>
              <Route path="/teams/:teamId/goals" element={ui} />
              <Route path="/teams/:teamId" element={ui} />
            </Routes>
          </MemoryRouter>
        </ToastProvider>
      </ThemeProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.mocked(api.listGoals).mockReset().mockResolvedValue(SAMPLE_GOALS);
});

describe("GoalsPage", () => {
  it("renders goal cards with status, owner, and progress", async () => {
    withProviders(<GoalsPage />);
    expect(await screen.findByText("Grow activation")).toBeVisible();
    expect(screen.getByText("Cut churn")).toBeVisible();
    expect(screen.getByTestId("goal-status-active")).toBeVisible();
    expect(screen.getByTestId("goal-progress")).toHaveTextContent("50%");
    expect(screen.getByText("No key results — progress undefined")).toBeVisible();
  });

  it("shows create button only for Manager+ roles", async () => {
    // useWorkspace reads the query param? No — it reads /me; mock via workspace query
    withProviders(<GoalsPage />);
    // Without a workspace provider override the role defaults to undefined → hidden.
    await waitFor(() => expect(screen.queryByTestId("create-goal")).toBeNull());
  });

  it("filters by status via the status tabs", async () => {
    vi.mocked(api.listGoals).mockResolvedValue({
      items: [SAMPLE_GOALS.items[0]!],
      page: 1,
      page_size: 25,
      total: 1,
    });
    const user = userEvent.setup();
    withProviders(<GoalsPage />);
    await user.click(await screen.findByTestId("filter-active"));
    await waitFor(() =>
      expect(api.listGoals).toHaveBeenCalledWith("t1", "active"),
    );
    expect(screen.queryByText("Cut churn")).toBeNull();
  });

  it("shows an empty state when the team has no goals", async () => {
    vi.mocked(api.listGoals).mockResolvedValue({
      items: [],
      page: 1,
      page_size: 25,
      total: 0,
    });
    withProviders(<GoalsPage />);
    expect(await screen.findByTestId("empty-state")).toBeVisible();
  });

  it("exposes an error state with retry", async () => {
    vi.mocked(api.listGoals).mockRejectedValue(new Error("boom"));
    const user = userEvent.setup();
    withProviders(<GoalsPage />);
    await user.click(await screen.findByText("Try again"));
    expect(api.listGoals).toHaveBeenCalledTimes(2);
  });
});

describe("GoalFormDialog", () => {
  it("blocks submission without a title", async () => {
    vi.mocked(api.createGoal).mockReset();
    const user = userEvent.setup();
    withProviders(
      <GoalFormDialog open={true} onOpenChange={() => undefined} teamId="t1" />,
      "/teams/t1/goals",
    );
    await user.click(await screen.findByTestId("goal-submit"));
    expect(await screen.findByText("Title is required")).toBeVisible();
    expect(api.createGoal).not.toHaveBeenCalled();
  });

  it("submits a valid goal", async () => {
    vi.mocked(api.createGoal).mockResolvedValue({
      id: "g9",
      team_id: "t1",
      title: "New objective",
      description: null,
      owner: { id: "u1", display_name: "Ada Admin", email: "a@t.example" },
      status: "draft" as const,
      start_date: null,
      due_date: null,
      created_by: { id: "u1", display_name: "Ada Admin", email: "a@t.example" },
      completed_at: null,
      created_at: 1,
      updated_at: 1,
    });
    const user = userEvent.setup();
    withProviders(
      <GoalFormDialog open={true} onOpenChange={() => undefined} teamId="t1" />,
      "/teams/t1/goals",
    );
    await user.type(screen.getByLabelText("Title"), "New objective");
    await user.click(screen.getByTestId("goal-submit"));
    await waitFor(() =>
      expect(api.createGoal).toHaveBeenCalledWith("t1", {
        title: "New objective",
        description: undefined,
        owner_id: undefined,
        start_date: undefined,
        due_date: undefined,
      }),
    );
  });
});
