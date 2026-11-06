import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as ApiClientModule from "@/api/client";
import {
  AuthProvider,
  emitSignedOut,
  handleUnauthorizedError,
  isSignedOut,
  resetSignedOut,
  useAuth,
  useLogout,
} from "@/lib/auth";
import { ApiError, api } from "@/api/client";

vi.mock("@/api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof ApiClientModule>();
  return {
    ...actual,
    api: {
      ...actual.api,
      me: vi.fn(),
      logout: vi.fn(),
    },
  };
});

const adminUser = {
  id: "u1",
  email: "admin@tasker.local",
  display_name: "Ada Admin",
  created_at: 1,
  mfa_enabled: false,
  capacity_limit: 10,
};

/** Controls whether the mocked /me endpoint currently fails (dead session). */
let meError: Error | null = null;

function freshClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
}

beforeEach(() => {
  resetSignedOut();
  meError = null;
  vi.mocked(api.me).mockReset();
  vi.mocked(api.logout).mockReset();
  vi.mocked(api.me).mockImplementation(() =>
    meError ? Promise.reject(meError) : Promise.resolve(adminUser),
  );
  vi.mocked(api.logout).mockResolvedValue(undefined);
});

describe("handleUnauthorizedError (global QueryCache.onError)", () => {
  it("ignores non-401 errors: no cache clear, no redirect", () => {
    const qc = freshClient();
    const clearSpy = vi.spyOn(qc, "clear");
    const navigate = vi.fn();

    handleUnauthorizedError(new ApiError(500, { status: 500, title: "boom" }), qc, navigate);
    handleUnauthorizedError(new ApiError(403, { status: 403, title: "no" }), qc, navigate);
    handleUnauthorizedError(new Error("network down"), qc, navigate);

    expect(clearSpy).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
    expect(isSignedOut()).toBe(false);
  });

  it("clears the cache and redirects to /login on a 401", () => {
    const qc = freshClient();
    const clearSpy = vi.spyOn(qc, "clear");
    const navigate = vi.fn();

    handleUnauthorizedError(new ApiError(401, { status: 401, title: "expired" }), qc, navigate);

    expect(clearSpy).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenCalledWith("/login");
    expect(isSignedOut()).toBe(true);
  });

  it("clears exactly once across repeated 401s (no refetch loop)", () => {
    resetSignedOut();
    const qc = freshClient();
    const clearSpy = vi.spyOn(qc, "clear");
    const navigate = vi.fn();
    const expired = () => new ApiError(401, { status: 401, title: "expired" });

    handleUnauthorizedError(expired(), qc, navigate);
    handleUnauthorizedError(expired(), qc, navigate);
    handleUnauthorizedError(expired(), qc, navigate);

    // The cache is torn down once; every 401 still lands on /login (a no-op
    // when already there — the router ignores same-URL navigations).
    expect(clearSpy).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenCalledTimes(3);
    for (const call of navigate.mock.calls) {
      expect(call[0]).toBe("/login");
    }
  });
});

describe("AuthProvider signed-out handling", () => {
  function AuthProbe() {
    const { user, isLoading } = useAuth();
    return (
      <div>
        {isLoading ? "loading" : user ? `user:${user.display_name}` : "anonymous"}
      </div>
    );
  }

  function renderAuth(qc: QueryClient) {
    return render(
      <QueryClientProvider client={qc}>
        <AuthProvider>
          <AuthProbe />
        </AuthProvider>
      </QueryClientProvider>,
    );
  }

  it("masks the cached user once the session is torn down", async () => {
    const qc = freshClient();
    renderAuth(qc);
    expect(await screen.findByText("user:Ada Admin")).toBeInTheDocument();

    // The session dies server-side: the next /me fails with 401.
    meError = new ApiError(401, { status: 401, title: "expired" });
    emitSignedOut();

    expect(await screen.findByText("anonymous")).toBeInTheDocument();
    // The mask is stable: no user element returns while torn down.
    // (Cache eviction + refetch behavior is covered by the logout test.)
    await waitFor(() => {
      expect(screen.getByText("anonymous")).toBeInTheDocument();
      expect(screen.queryByText("user:Ada Admin")).not.toBeInTheDocument();
    });
  });

  it("re-arms the mask on a second teardown after a re-login", async () => {
    const qc = freshClient();
    renderAuth(qc);
    expect(await screen.findByText("user:Ada Admin")).toBeInTheDocument();

    // First teardown, then a successful re-login (fresh /me data).
    meError = new ApiError(401, { status: 401, title: "expired" });
    emitSignedOut();
    expect(await screen.findByText("anonymous")).toBeInTheDocument();
    meError = null;
    await qc.invalidateQueries();
    expect(await screen.findByText("user:Ada Admin")).toBeInTheDocument();

    // Second teardown must mask again — a stale teardown timestamp from the
    // first 401 must not disable it (regression: logout bounce loop).
    meError = new ApiError(401, { status: 401, title: "expired" });
    emitSignedOut();
    expect(await screen.findByText("anonymous")).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByText("anonymous")).toBeInTheDocument();
      expect(screen.queryByText("user:Ada Admin")).not.toBeInTheDocument();
    });
  });

  it("trusts fresh user data again after a new session is established", async () => {
    const qc = freshClient();
    renderAuth(qc);
    expect(await screen.findByText("user:Ada Admin")).toBeInTheDocument();

    meError = new ApiError(401, { status: 401, title: "expired" });
    emitSignedOut();
    expect(await screen.findByText("anonymous")).toBeInTheDocument();

    // Sign-in flow: the session works again and the query is invalidated.
    meError = null;
    await qc.invalidateQueries();
    expect(await screen.findByText("user:Ada Admin")).toBeInTheDocument();
  });
});

describe("useLogout", () => {
  function LogoutProbe() {
    const { user } = useAuth();
    const logout = useLogout();
    return (
      <div>
        <span data-testid="auth-state">{user ? "authed" : "anonymous"}</span>
        <button type="button" onClick={() => logout.mutate()}>
          log out
        </button>
      </div>
    );
  }

  function renderLogoutFlow(qc: QueryClient) {
    return render(
      <QueryClientProvider client={qc}>
        <AuthProvider>
          <MemoryRouter initialEntries={["/app"]}>
            <Routes>
              <Route path="/app" element={<LogoutProbe />} />
              <Route path="/login" element={<div data-testid="login-page">Sign in</div>} />
            </Routes>
          </MemoryRouter>
        </AuthProvider>
      </QueryClientProvider>,
    );
  }

  it("clears the cache, masks the user, and lands on /login", async () => {
    const qc = freshClient();
    const user = userEvent.setup();
    renderLogoutFlow(qc);
    expect(await screen.findByText("authed")).toBeInTheDocument();

    // Reality after logout: /me rejects until a new sign-in.
    vi.mocked(api.logout).mockImplementation(() => {
      meError = new ApiError(401, { status: 401, title: "expired" });
      return Promise.resolve(undefined);
    });

    await user.click(screen.getByRole("button", { name: "log out" }));

    expect(await screen.findByTestId("login-page")).toBeInTheDocument();
    // The login page guard sees no user: no bounce back to the app.
    await waitFor(() => expect(screen.getByTestId("login-page")).toBeInTheDocument(), {
      timeout: 500,
    });
    expect(screen.queryByText("authed")).not.toBeInTheDocument();
    expect(screen.queryByTestId("auth-state")).not.toBeInTheDocument();
    // The cache was cleared and the post-clear /me refetch failed with 401.
    await waitFor(() => expect(qc.getQueryData(["me"])).toBeUndefined());
  });
});
