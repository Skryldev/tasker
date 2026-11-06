import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import type * as apiClientModule from "@/api/client";
import { api } from "@/api/client";
import { AuthProvider } from "@/lib/auth";
import { ToastProvider } from "@/components/ui/toast";
import { WorkspaceProvider } from "@/lib/workspace";
import { TaskDialog } from "@/components/app/TaskDialog";
import { GoalFormDialog } from "@/pages/GoalsPage";
import { AccountPage } from "@/pages/AccountPage";

/**
 * Regression test for the reporter/assignee "you" bug: every surface that
 * renders the current user must show the record's display name. A "(you)"
 * suffix is allowed as an additive marker — never as a replacement.
 */

vi.mock("@/api/client", async (importOriginal) => {
  const original = (await importOriginal()) as typeof apiClientModule;
  return {
    ...original,
    api: {
      ...original.api,
      me: vi.fn(),
      listMembers: vi.fn(),
      createTask: vi.fn(),
      listTeams: vi.fn(),
      mySummary: vi.fn(),
      listSessions: vi.fn(),
      changePassword: vi.fn(),
      mfaSetup: vi.fn(),
      updateProfile: vi.fn(),
    },
  };
});

const currentUser = {
  id: "u-you",
  email: "amir@tasker.local",
  display_name: "Amir Karimi",
  username: "amir",
  bio: null,
  timezone: null,
  created_at: 1,
  mfa_enabled: false,
  capacity_limit: 10,
  notification_prefs: {
    task_assigned: true,
    comment_mention: true,
    task_submitted: true,
    task_accepted: true,
    task_rejected: true,
    task_status_changed: true,
  },
};

const members = {
  items: [
    {
      user: { id: "u-you", display_name: "Amir Karimi", email: "amir@tasker.local" },
      username: "amir",
      role: "owner" as const,
      created_at: 1,
    },
    {
      user: { id: "u-2", display_name: "Sara Ahmadi", email: "sara@tasker.local" },
      username: "sara",
      role: "member" as const,
      created_at: 1,
    },
  ],
};

function renderWithAuth(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/?team=t1"]}>
        <WorkspaceProvider>
          <AuthProvider>
            <ToastProvider>{ui}</ToastProvider>
          </AuthProvider>
        </WorkspaceProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  // GoalFormDialog reads members with a raw fetch; stub it for jsdom.
  vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
    const url = String(input);
    if (url.includes("/members")) {
      return Promise.resolve(
        new Response(JSON.stringify(members), { status: 200 }),
      );
    }
    return Promise.reject(new Error(`unstubbed fetch: ${url}`));
  });
  vi.mocked(api.me).mockReset();
  vi.mocked(api.me).mockResolvedValue(currentUser);
  vi.mocked(api.listMembers).mockReset();
  vi.mocked(api.listMembers).mockResolvedValue(members);
  vi.mocked(api.listTeams).mockResolvedValue({
    items: [
      {
        id: "t1",
        name: "Team One",
        description: null,
        timezone: null,
        archived_at: null,
        created_at: 1,
        updated_at: 1,
        my_role: "owner",
      },
    ],
  });
  vi.mocked(api.mySummary).mockResolvedValue({
    by_status: { backlog: 0, todo: 0, in_progress: 0, in_review: 0, resolved: 0, closed: 0 },
    overdue: 0,
    unread_mentions_7d: 0,
  });
  vi.mocked(api.listSessions).mockResolvedValue({ items: [] });
});

afterEach(cleanup);

describe("current user renders with display name (you-bug regression)", () => {
  it("task dialog reporter option shows the display name with an additive (you)", async () => {
    const user = userEvent.setup();
    renderWithAuth(<TaskDialog open onOpenChange={() => undefined} teamId="t1" projectId="p1" />);
    // Open the reporter select and inspect the creator option. The select
    // renders once the workspace role resolves.
    await user.click(await screen.findByLabelText("Reporter"));
    const option = await screen.findByRole("option", { name: /Amir Karimi \(you\)/ });
    expect(option).toBeInTheDocument();
    // The display name must be present — a bare "Me" label is the bug.
    expect(screen.queryByRole("option", { name: /^Me\b/ })).toBeNull();
  });

  it("goal form owner option shows the display name with an additive (you)", async () => {
    const user = userEvent.setup();
    renderWithAuth(<GoalFormDialog open onOpenChange={() => undefined} teamId="t1" />);
    await screen.findByTestId("goal-form");
    await user.click(await screen.findByLabelText("Owner"));
    const option = await screen.findByRole("option", { name: /Amir Karimi \(you\)/ });
    expect(option).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /^Me$/ })).toBeNull();
  });

  it("account page shows the display name, not a you/me label", async () => {
    renderWithAuth(<AccountPage />);
    expect(await screen.findByDisplayValue("Amir Karimi")).toBeInTheDocument();
    // No bare "you"/"me" text node may replace the identity anywhere.
    expect(screen.queryByText("you")).toBeNull();
    expect(screen.queryByText("Me")).toBeNull();
  });
});
