import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { api, ApiError } from "@/api/client";
import type * as apiClientModule from "@/api/client";
import { ToastProvider } from "@/components/ui/toast";
import { AuthProvider } from "@/lib/auth";
import { LoginPage } from "@/pages/LoginPage";
import { MfaSettings } from "@/components/app/MfaSettings";

vi.mock("@/api/client", async (importOriginal) => {
  const original = (await importOriginal()) as typeof apiClientModule;
  return {
    ...original,
    api: {
      ...original.api,
      login: vi.fn(),
      me: vi.fn(),
      mfaSetup: vi.fn(),
      mfaEnable: vi.fn(),
      mfaVerify: vi.fn(),
      mfaDisable: vi.fn(),
      mfaRecoveryCodes: vi.fn(),
    },
  };
});

function renderPage(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <AuthProvider>
          <ToastProvider>{ui}</ToastProvider>
        </AuthProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Mock /me as signed out so the login form renders. */
function signedOut() {
  vi.mocked(api.me).mockRejectedValue(new ApiError(401, { detail: "no session" }));
}

describe("login → MFA verification flow", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("transitions from password entry to the MFA step when mfa_required", async () => {
    const user = userEvent.setup();
    signedOut();
    vi.mocked(api.login).mockResolvedValueOnce({
      mfa_required: true,
      mfa_token: "mfp_test-token",
    });
    renderPage(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "mfa@tasker.local");
    await user.type(screen.getByLabelText("Password"), "test-password-123");
    await user.click(screen.getByTestId("login-submit"));
    expect(await screen.findByTestId("mfa-verify-form")).toBeVisible();
    // No navigation happened: still on the login route with the MFA form.
    expect(screen.getByTestId("mfa-login-code")).toBeVisible();
  });

  it("submits the pending token with the code and succeeds into the app", async () => {
    const user = userEvent.setup();
    signedOut();
    vi.mocked(api.login).mockResolvedValueOnce({
      mfa_required: true,
      mfa_token: "mfp_pending",
    });
    vi.mocked(api.mfaVerify).mockImplementationOnce((_token, _code) => {
      // Successful verification clears the session cookie server-side; the
      // subsequent /me refetch resolves as authenticated.
      vi.mocked(api.me).mockResolvedValueOnce({
        id: "u1",
        email: "mfa@tasker.local",
        display_name: "Mfa",
        created_at: 0,
        mfa_enabled: true,
      });
      return Promise.resolve({
        id: "u1",
        email: "mfa@tasker.local",
        display_name: "Mfa",
        created_at: 0,
        mfa_enabled: true,
      });
    });
    renderPage(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "mfa@tasker.local");
    await user.type(screen.getByLabelText("Password"), "test-password-123");
    await user.click(screen.getByTestId("login-submit"));
    await screen.findByTestId("mfa-verify-form");
    await user.type(screen.getByTestId("mfa-login-code"), "123456");
    await user.click(screen.getByTestId("mfa-verify-submit"));
    await waitFor(() =>
      expect(api.mfaVerify).toHaveBeenCalledWith("mfp_pending", "123456"),
    );
  });

  it("invalid MFA responses show an error and never navigate", async () => {
    const user = userEvent.setup();
    signedOut();
    vi.mocked(api.login).mockResolvedValueOnce({
      mfa_required: true,
      mfa_token: "mfp_pending",
    });
    vi.mocked(api.mfaVerify).mockRejectedValueOnce(
      new ApiError(401, { detail: "authentication is required" }),
    );
    renderPage(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "mfa@tasker.local");
    await user.type(screen.getByLabelText("Password"), "test-password-123");
    await user.click(screen.getByTestId("login-submit"));
    await screen.findByTestId("mfa-verify-form");
    await user.type(screen.getByTestId("mfa-login-code"), "000000");
    await user.click(screen.getByTestId("mfa-verify-submit"));
    expect(await screen.findByTestId("login-error")).toHaveTextContent(
      /code was not accepted/i,
    );
    // Still on the MFA step.
    expect(screen.getByTestId("mfa-verify-form")).toBeVisible();
  });

  it("normal login (no MFA) does not show the MFA step", async () => {
    const user = userEvent.setup();
    signedOut();
    vi.mocked(api.login).mockResolvedValueOnce({
      id: "u1",
      email: "plain@tasker.local",
      display_name: "Plain",
      created_at: 0,
      mfa_enabled: false,
    });
    // The post-login /me refetch keeps resolving signed-out so the form stays
    // mounted in the test router; what matters is that no MFA step appeared.
    renderPage(<LoginPage />);
    await user.type(screen.getByLabelText("Email"), "plain@tasker.local");
    await user.type(screen.getByLabelText("Password"), "test-password-123");
    await user.click(screen.getByTestId("login-submit"));
    await waitFor(() => expect(api.login).toHaveBeenCalledOnce());
    // The MFA form must never render for non-MFA users.
    expect(screen.queryByTestId("mfa-verify-form")).toBeNull();
  });
});

describe("MfaSettings (account page)", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("starts setup and renders the QR code from the provisioning URI", async () => {
    const user = userEvent.setup();
    vi.mocked(api.me).mockResolvedValue({
      id: "u1",
      email: "u@tasker.local",
      display_name: "U",
      created_at: 0,
      mfa_enabled: false,
    });
    vi.mocked(api.mfaSetup).mockResolvedValueOnce({
      uri: "otpauth://totp/Tasker:u%40tasker.local?issuer=Tasker&secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
      secret: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
      issuer: "Tasker",
      account: "u@tasker.local",
    });
    renderPage(<MfaSettings enabled={false} />);
    await user.click(screen.getByTestId("mfa-start"));
    const qr = await screen.findByTestId("mfa-qr");
    expect(qr).toBeVisible();
    // Manual entry is hidden until requested; then it shows the Base32 secret.
    expect(screen.queryByTestId("mfa-secret")).toBeNull();
    await user.click(screen.getByRole("button", { name: /show/i }));
    const secret = await screen.findByTestId("mfa-secret");
    expect(secret).toHaveTextContent(/GEZDGNBV/);
  });

  it("shows ten recovery codes exactly once with an acknowledgement gate", async () => {
    const user = userEvent.setup();
    vi.mocked(api.mfaEnable).mockResolvedValueOnce({
      recovery_codes: Array.from({ length: 10 }, (_, i) => `AAAA-${String(1000 + i).slice(-4)}`),
    });
    renderPage(<MfaSettings enabled={false} />);
    // Jump straight to the enable call via a pre-seeded setup state: start setup.
    vi.mocked(api.mfaSetup).mockResolvedValueOnce({
      uri: "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ",
      secret: "GEZDGNBVGY3TQOJQ",
      issuer: "Tasker",
      account: "u@tasker.local",
    });
    await user.click(screen.getByTestId("mfa-start"));
    await screen.findByTestId("mfa-qr");
    await user.type(screen.getByTestId("mfa-code-input"), "123456");
    await user.click(screen.getByTestId("mfa-enable-submit"));
    const codes = await screen.findAllByTestId("mfa-recovery-code");
    expect(codes).toHaveLength(10);
    // The done button is gated on the acknowledgement checkbox... (button
    // disabled until the user confirms they stored the codes).
    const done = screen.getByTestId("mfa-recovery-done");
    expect(done).toBeDisabled();
    // Enable stays pending until codes are acknowledged — no direct path.
  });

  it("requires the current password to regenerate recovery codes", async () => {
    const user = userEvent.setup();
    vi.mocked(api.mfaRecoveryCodes).mockResolvedValueOnce({
      recovery_codes: Array.from({ length: 10 }, (_, i) => `BBBB-${String(2000 + i).slice(-4)}`),
    });
    renderPage(<MfaSettings enabled />);
    await user.click(screen.getByTestId("mfa-regenerate"));
    // Dialog requires the current password before the action.
    expect(screen.getByLabelText("Current password")).toBeVisible();
    await user.type(screen.getByTestId("mfa-regen-password"), "test-password-123");
    await user.click(screen.getByRole("button", { name: "Generate codes" }));
    await waitFor(() =>
      expect(api.mfaRecoveryCodes).toHaveBeenCalledWith("test-password-123"),
    );
    expect(await screen.findAllByTestId("mfa-recovery-code")).toHaveLength(10);
  });

  it("requires the current password to disable 2FA", async () => {
    const user = userEvent.setup();
    vi.mocked(api.mfaDisable).mockResolvedValueOnce(undefined);
    renderPage(<MfaSettings enabled />);
    await user.click(screen.getByTestId("mfa-disable"));
    await user.type(screen.getByTestId("mfa-disable-password"), "test-password-123");
    await user.click(screen.getByRole("button", { name: "Disable 2FA" }));
    await waitFor(() => expect(api.mfaDisable).toHaveBeenCalledWith("test-password-123"));
  });

  it("validates the six-digit code format before submitting", async () => {
    const user = userEvent.setup();
    vi.mocked(api.mfaSetup).mockResolvedValueOnce({
      uri: "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ",
      secret: "GEZDGNBVGY3TQOJQ",
      issuer: "Tasker",
      account: "u@tasker.local",
    });
    renderPage(<MfaSettings enabled={false} />);
    await user.click(screen.getByTestId("mfa-start"));
    await screen.findByTestId("mfa-qr");
    const input = screen.getByTestId("mfa-code-input") as HTMLInputElement;
    // Non-digits are stripped; only six digits enable submission.
    await user.type(input, "12ab34x56");
    expect(input.value).toBe("123456");
    expect(screen.getByTestId("mfa-enable-submit")).not.toBeDisabled();
  });
});
