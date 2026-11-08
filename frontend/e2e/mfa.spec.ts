import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { currentStep, totpAtStep, totpNow, waitMsForFreshStep } from "./totp";

/**
 * Phase 7 E2E: TOTP two-factor authentication.
 *
 * The TOTP codes here are computed independently (Node crypto, HMAC-SHA-1,
 * RFC 6238) from the Base32 secret the setup endpoint returns — the same
 * computation any authenticator app performs.
 *
 * Timing discipline: the backend accepts only the current 30-second step
 * (window 0). Codes are therefore generated immediately before submission
 * and only when at least 5 seconds of the step remain; a retry always
 * generates a code for a NEW step (never re-submits a potentially expired
 * one). Replay protection and enrollment semantics are not weakened.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.describe.serial("Phase 7 two-factor authentication", () => {
  let context: BrowserContext;
  let page: Page;
  let secret = "";
  /** TOTP step consumed by the latest successful verification. */
  let consumedStep = -1;

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  });

  test.afterAll(async () => {
    await context.close();
  });

  test("setup shows a locally rendered QR code and manual secret", async () => {
    await page.goto("/account");
    await page.getByTestId("mfa-start").click();
    await expect(page.getByTestId("mfa-qr")).toBeVisible();
    const uri = await page.evaluate(
      () => document.querySelector('[data-testid="mfa-qr"] svg')?.getAttribute("value") ?? null,
    );
    // QRCodeSVG does not carry the value attribute; assert via the secret row.
    await page.getByRole("button", { name: /show/i }).click();
    secret = (await page.getByTestId("mfa-secret").textContent()) ?? "";
    expect(secret).toMatch(/^[A-Z2-7]+$/);
    expect(uri === null || uri.includes("otpauth")).toBe(true);
  });

  test("enabling with the current TOTP code reveals ten recovery codes once", async () => {
    /**
     * One bounded enrollment attempt. Returns "ok" on success, "timing" when
     * the two-step window likely expired (retry with a fresh code), or
     * "app" when the application rejected a fresh code outright.
     */
    const tryEnable = async (): Promise<"ok" | "timing" | "app"> => {
      // Never generate a code for a step that is about to roll over.
      const settle = waitMsForFreshStep();
      if (settle > 0) {
        await page.waitForTimeout(settle);
      }
      // Compute as close to submission as practical.
      const code = totpNow(secret);
      await page.getByTestId("mfa-code-input").fill(code);
      await page.getByTestId("mfa-enable-submit").click();
      // Success replaces the form with the recovery panel; a rejection keeps
      // the form mounted with an error banner.
      try {
        await page
          .getByTestId("mfa-recovery")
          .waitFor({ state: "visible", timeout: 5_000 });
        return "ok";
      } catch {
        const bannerVisible = await page
          .locator("[role=alert], .text-destructive")
          .first()
          .isVisible()
          .catch(() => false);
        return bannerVisible ? "app" : "timing";
      }
    };

    let outcome = await tryEnable();
    if (outcome === "timing") {
      // Retry ONCE with a freshly generated code for the new step.
      outcome = await tryEnable();
    }
    if (outcome === "timing") {
      throw new Error(
        "MFA enrollment did not complete within two TOTP steps — " +
          "code timing exhausted (this is a test-boundary issue, not an app rejection)",
      );
    }
    expect(outcome, "enrollment succeeded (app accepted a fresh TOTP code)").toBe(
      "ok",
    );

    const codes = await page.getByTestId("mfa-recovery-code").allTextContents();
    expect(codes).toHaveLength(10);
    for (const c of codes) {
      expect(c).toMatch(/^[23456789A-Z]{4}-[23456789A-Z]{4}$/);
    }
    // Done is gated on the acknowledgement checkbox.
    await expect(page.getByTestId("mfa-recovery-done")).toBeDisabled();
    await page.getByTestId("mfa-recovery-ack").check();
    await page.getByTestId("mfa-recovery-done").click();
    await expect(page.getByTestId("mfa-enabled")).toBeVisible();
  });

  test("MFA login requires the TOTP code and issues the session", async () => {
    // Fresh page; clear cookies so the session from the enable test is gone
    // (deterministic — no reliance on the UI logout redirect timing).
    await context.clearCookies();
    const mp = await context.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(ADMIN.email);
    await mp.getByLabel("Password").fill(ADMIN.password);
    await mp.getByTestId("login-submit").click();
    await expect(mp.getByTestId("mfa-verify-form")).toBeVisible();
    // No authenticated page was reached: the URL is still /login.
    expect(new URL(mp.url()).pathname).toBe("/login");

    // Boundary guard, then compute immediately before submitting.
    const settle = waitMsForFreshStep();
    if (settle > 0) {
      await mp.waitForTimeout(settle);
    }
    await mp
      .getByTestId("mfa-login-code")
      .fill(totpAtStep(secret, currentStep()));
    await mp.getByTestId("mfa-verify-submit").click();
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    // Record the step the server just consumed (replay protection pins it).
    consumedStep = currentStep();
    // Clear cookies so the next test starts from a clean login state.
    await context.clearCookies();
    await mp.close();
  });

  test("an invalid TOTP code is rejected without a session", async () => {
    const mp = await context.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(ADMIN.email);
    await mp.getByLabel("Password").fill(ADMIN.password);
    await mp.getByTestId("login-submit").click();
    await expect(mp.getByTestId("mfa-verify-form")).toBeVisible();
    await mp.getByTestId("mfa-login-code").fill("000000");
    await mp.getByTestId("mfa-verify-submit").click();
    await expect(mp.getByTestId("login-error")).toContainText(/not accepted/i);
    // Still on /login — no session was issued.
    expect(new URL(mp.url()).pathname).toBe("/login");

    // Clean up the failed attempt with a valid code so later steps stay sane.
    // Test 3 consumed step `consumedStep` and replay protection pins it
    // (window 0), so if we are still inside that step, wait for the next
    // boundary; then submit a code computed for the fresh current step.
    if (consumedStep >= 0 && currentStep() <= consumedStep) {
      const waitMs = (consumedStep + 1) * 30_000 - Date.now() + 500;
      await mp.waitForTimeout(Math.max(waitMs, 1_000));
    }
    expect(
      currentStep(),
      "test invariant: cleanup runs in a step after the consumed one",
    ).toBeGreaterThan(consumedStep);
    await mp
      .getByTestId("mfa-login-code")
      .fill(totpNow(secret));
    await mp.getByTestId("mfa-verify-submit").click();
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    consumedStep = currentStep();
    await mp.close();
  });

  test("disable requires the current password and returns to normal login", async () => {
    const mp = await context.newPage();
    await mp.goto("/account");
    await mp.getByTestId("mfa-disable").click();
    // Wrong password first: rejected, MFA stays enabled.
    await mp.getByTestId("mfa-disable-password").fill("wrong-password-1");
    await mp.getByRole("button", { name: "Disable 2FA" }).click();
    await expect(mp.getByRole("alert").last()).toBeVisible();
    // Correct password disables.
    await mp.getByTestId("mfa-disable-password").fill(ADMIN.password);
    await mp.getByRole("button", { name: "Disable 2FA" }).click();
    await expect(mp.getByTestId("mfa-start")).toBeVisible();
    // Login no longer asks for a code.
    await context.clearCookies();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(ADMIN.email);
    await mp.getByLabel("Password").fill(ADMIN.password);
    await mp.getByTestId("login-submit").click();
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await mp.close();
  });
});
