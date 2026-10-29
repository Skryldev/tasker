import { useState } from "react";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { api } from "@/api/client";
import { CommentBody, CommentComposer } from "@/components/app/Mentions";
import { NotificationBell } from "@/components/app/NotificationBell";
import { ToastProvider } from "@/components/ui/toast";
import { ThemeProvider } from "@/lib/theme";

vi.mock("@/api/client", async (importOriginal) => {
  const original = (await importOriginal()) as Record<string, unknown>;
  return { ...original, api: { ...(original.api as object) } };
});

vi.mock("@/api/hooks", () => ({
  useMembers: () => ({
    data: {
      items: [
        { user: { id: "u1", display_name: "Mona Member", email: "mona@test.example" }, role: "member" },
        { user: { id: "u2", display_name: "Ada Admin", email: "ada@test.example" }, role: "owner" },
      ],
    },
    isLoading: false,
  }),
}));

function withProviders(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ThemeProvider>
        <ToastProvider>
          <MemoryRouter>{ui}</MemoryRouter>
        </ToastProvider>
      </ThemeProvider>
    </QueryClientProvider>,
  );
}

describe("CommentBody", () => {
  it("highlights known team mentions by email", () => {
    withProviders(<CommentBody teamId="t1" body="ping @mona@test.example please" />);
    expect(screen.getByText("@Mona Member")).toBeVisible();
    expect(screen.queryByText("@mona@test.example")).toBeNull();
  });

  it("renders unknown addresses as plain text", () => {
    withProviders(<CommentBody teamId="t1" body="ping @stranger@nowhere.example" />);
    expect(screen.getByText(/@stranger@nowhere\.example/)).toBeVisible();
  });
});

describe("CommentComposer", () => {
  beforeEach(() => {
    vi.mocked(api).listNotifications = vi.fn();
  });

  function StatefulComposer() {
    const [value, setValue] = useState("");
    return (
      <CommentComposer
        teamId="t1"
        value={value}
        onChange={setValue}
        onSubmit={() => undefined}
      />
    );
  }

  it("opens the member list after typing @ and inserts the email token", async () => {
    const user = userEvent.setup();
    withProviders(<StatefulComposer />);
    const input = screen.getByTestId("comment-input");
    await user.type(input, "hello @mon");
    expect(await screen.findByTestId("mention-list")).toBeVisible();
    await user.click(screen.getByText("Mona Member"));
    expect(input).toHaveValue("hello @mona@test.example ");
  });

  it("does not open the list without an @", async () => {
    const user = userEvent.setup();
    withProviders(
      <CommentComposer teamId="t1" value="" onChange={() => undefined} onSubmit={() => undefined} />,
    );
    await user.type(screen.getByTestId("comment-input"), "plain text mona");
    expect(screen.queryByTestId("mention-list")).toBeNull();
  });
});

describe("NotificationBell", () => {
  beforeEach(() => {
    vi.mocked(api).unreadCount = vi.fn().mockResolvedValue({ count: 2 });
    vi.mocked(api).listNotifications = vi.fn().mockResolvedValue({
      items: [
        {
          id: "n1",
          event: "task_submitted",
          task_id: "t-1",
          actor: { id: "a1", display_name: "Mona Member", email: "mona@test.example" },
          summary: 'Mona Member submitted "Fix hero" for your review',
          created_at: Date.now() - 60_000,
          read_at: null,
        },
        {
          id: "n2",
          event: "task_accepted",
          task_id: "t-1",
          actor: { id: "a2", display_name: "Ada Admin", email: "ada@test.example" },
          summary: 'Ada Admin accepted your work on "Fix hero"',
          created_at: Date.now() - 120_000,
          read_at: Date.now() - 60_000,
        },
      ],
      page: 1,
      page_size: 20,
      total: 2,
    });
    vi.mocked(api).markNotificationRead = vi.fn().mockResolvedValue(undefined);
    vi.mocked(api).markAllNotificationsRead = vi.fn().mockResolvedValue(undefined);
  });

  it("shows the unread badge count", async () => {
    withProviders(<NotificationBell />);
    expect(await screen.findByTestId("unread-badge")).toHaveTextContent("2");
  });

  it("lists notifications with unread indicators and marks one read on open", async () => {
    const user = userEvent.setup();
    withProviders(<NotificationBell />);
    await user.click(await screen.findByTestId("notification-bell"));
    expect(await screen.findByTestId("notification-list")).toBeVisible();
    expect(screen.getByText('Mona Member submitted "Fix hero" for your review')).toBeVisible();
    await user.click(screen.getByText('Mona Member submitted "Fix hero" for your review'));
    await waitFor(() =>
      expect(vi.mocked(api).markNotificationRead).toHaveBeenCalledWith("n1"),
    );
  });

  it("marks all read from the header control", async () => {
    const user = userEvent.setup();
    withProviders(<NotificationBell />);
    await user.click(await screen.findByTestId("notification-bell"));
    await user.click(screen.getByTestId("mark-all-read"));
    await waitFor(() => expect(vi.mocked(api).markAllNotificationsRead).toHaveBeenCalled());
  });

  it("shows an empty state for zero notifications", async () => {
    vi.mocked(api).unreadCount = vi.fn().mockResolvedValue({ count: 0 });
    vi.mocked(api).listNotifications = vi.fn().mockResolvedValue({
      items: [],
      page: 1,
      page_size: 20,
      total: 0,
    });
    const user = userEvent.setup();
    withProviders(<NotificationBell />);
    await user.click(await screen.findByTestId("notification-bell"));
    expect(await screen.findByTestId("empty-state")).toBeVisible();
    expect(screen.queryByTestId("unread-badge")).toBeNull();
  });

  it("exposes an error state with retry", async () => {
    vi.mocked(api).listNotifications = vi.fn().mockRejectedValue(new Error("boom"));
    const user = userEvent.setup();
    withProviders(<NotificationBell />);
    await user.click(await screen.findByTestId("notification-bell"));
    expect(await screen.findByRole("alert")).toBeVisible();
    await user.click(screen.getByText("Retry"));
    expect(vi.mocked(api).listNotifications).toHaveBeenCalledTimes(2);
  });
});
