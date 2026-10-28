import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { api, ApiError } from "@/api/client";
import { LoginPage } from "@/pages/LoginPage";
import { ThemeProvider, useTheme } from "@/lib/theme";
import { ToastProvider, useToast } from "@/components/ui/toast";
import { PriorityBadge, StatusBadge } from "@/components/app/TaskBadges";
import { EmptyState, ErrorState } from "@/components/app/EmptyState";

// api.login is network-bound; tests replace it entirely.
vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/api/client")>();
  return { ...original, api: { ...original.api, login: vi.fn() } };
});

function renderWithProviders(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/login"]}>
        <ToastProvider>{ui}</ToastProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("LoginForm", () => {
  beforeEach(() => {
    vi.mocked(api.login).mockReset();
  });

  it("shows validation errors for empty fields", async () => {
    const user = userEvent.setup();
    renderWithProviders(<LoginPage />);
    await user.click(screen.getByTestId("login-submit"));
    expect(await screen.findByText("Email is required")).toBeVisible();
    expect(api.login).not.toHaveBeenCalled();
  });

  it("rejects invalid email format", async () => {
    const user = userEvent.setup();
    renderWithProviders(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "not-an-email");
    await user.type(screen.getByLabelText("Password"), "whatever");
    await user.click(screen.getByTestId("login-submit"));
    expect(await screen.findByText("Enter a valid email address")).toBeVisible();
  });

  it("surfaces wrong credentials from the API problem", async () => {
    const user = userEvent.setup();
    vi.mocked(api.login).mockRejectedValueOnce(
      new ApiError(401, { detail: "authentication is required" }),
    );
    renderWithProviders(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "admin@tasker.local");
    await user.type(screen.getByLabelText("Password"), "bad-password");
    await user.click(screen.getByTestId("login-submit"));
    expect(await screen.findByTestId("login-error")).toHaveTextContent(
      "Incorrect email or password.",
    );
  });

  it("rate-limit problems show the retry hint", async () => {
    const user = userEvent.setup();
    vi.mocked(api.login).mockRejectedValueOnce(
      new ApiError(429, { detail: "request rate limit exceeded" }),
    );
    renderWithProviders(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "admin@tasker.local");
    await user.type(screen.getByLabelText("Password"), "whatever-pass");
    await user.click(screen.getByTestId("login-submit"));
    expect(await screen.findByTestId("login-error")).toHaveTextContent("Too many attempts");
  });

  it("submits on success", async () => {
    const user = userEvent.setup();
    vi.mocked(api.login).mockResolvedValueOnce({
      id: "u1",
      email: "admin@tasker.local",
      display_name: "Ada",
      created_at: 0,
    });
    renderWithProviders(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "admin@tasker.local");
    await user.type(screen.getByLabelText("Password"), "good-password");
    await user.click(screen.getByTestId("login-submit"));
    await waitFor(() => expect(api.login).toHaveBeenCalledWith("admin@tasker.local", "good-password"));
  });
});

describe("task badges", () => {
  it("renders status labels", () => {
    render(<StatusBadge status="in_progress" />);
    expect(screen.getByText("In Progress")).toBeVisible();
  });

  it("renders priority with level bars", () => {
    render(<PriorityBadge priority="urgent" />);
    expect(screen.getByTitle("Priority: Urgent")).toBeVisible();
  });
});

describe("application states", () => {
  it("empty state shows title, description and action", () => {
    render(
      <EmptyState
        icon={<span />}
        title="Nothing here"
        description="Try something else"
        action={<button type="button">Create</button>}
      />,
    );
    expect(screen.getByTestId("empty-state")).toHaveTextContent("Nothing here");
    expect(screen.getByRole("button", { name: "Create" })).toBeVisible();
  });

  it("error state exposes an alert with retry", async () => {
    const onRetry = vi.fn();
    const user = userEvent.setup();
    render(<ErrorState title="Boom" detail="db down" onRetry={onRetry} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Boom");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });
});

describe("theme switching", () => {
  function Probe() {
    const { theme, toggle } = useTheme();
    return (
      <button type="button" onClick={toggle}>
        {theme}
      </button>
    );
  }

  it("toggles the dark class on the document element", async () => {
    const user = userEvent.setup();
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    const button = screen.getByRole("button");
    const initial = button.textContent;
    await user.click(button);
    expect(button.textContent).toBe(initial === "dark" ? "light" : "dark");
    expect(document.documentElement.classList.contains("dark")).toBe(
      button.textContent === "dark",
    );
  });
});

describe("toasts", () => {
  function Trigger() {
    const { push } = useToast();
    return (
      <button type="button" onClick={() => push("error", "It failed", "with details")}>
        fail
      </button>
    );
  }

  it("renders pushed toasts with a dismiss control", async () => {
    const user = userEvent.setup();
    render(
      <ToastProvider>
        <Trigger />
      </ToastProvider>,
    );
    await user.click(screen.getByRole("button", { name: "fail" }));
    expect(await screen.findByText("It failed")).toBeVisible();
    expect(screen.getByText("with details")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Dismiss notification" }));
    await waitFor(() => expect(screen.queryByText("It failed")).not.toBeInTheDocument());
  });
});
