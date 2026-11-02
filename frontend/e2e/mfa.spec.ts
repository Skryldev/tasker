import { createHmac } from "node:crypto";
import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 7 E2E: TOTP two-factor authentication.
 *
 * The TOTP codes here are computed independently (Node crypto, HMAC-SHA-1,
 * RFC 6238) from the Base32 secret the setup endpoint returns — the same
 * computation any authenticator app performs.
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

function base32Decode(input: string): Buffer {
  let bits = 0;
  let value = 0;
  const out: number[] = [];
  for (const c of input.replace(/=+$/, "").toUpperCase()) {
    const idx = BASE32.indexOf(c);
    if (idx < 0) continue;
    value = (value << 5) | idx;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}

function totpAt(secretBase32: string, step: number): string {
  const key = base32Decode(secretBase32);
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", key).update(counter).digest();
  const offset = digest[digest.length - 1] & 0x0f;
  const binary =
    ((digest[offset] & 0x7f) << 24) |
    (digest[offset + 1] << 16) |
    (digest[offset + 2] << 8) |
    digest[offset + 3];
  return String(binary % 1_000_000).padStart(6, "0");
}

const currentStep = () => Math.floor(Date.now() / 1000 / 30);

test.describe.serial("Phase 7 two-factor authentication", () => {
  let context: BrowserContext;
  let page: Page;
  let secret = "";

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
    // Retry once across a 30-second step boundary: the backend accepts only
    // the current step (window 0), so a code computed just before a boundary
    // roll is legitimately stale by the time the server sees it.
    const tryEnable = async () => {
      await page.getByTestId("mfa-code-input").fill(totpAt(secret, currentStep()));
      await page.getByTestId("mfa-enable-submit").click();
      // Recovery codes replace the form on success; an invalid code keeps
      // the form mounted with an error banner. Wait for the panel ONLY —
      // the code input races the re-render in the instant after success.
      try {
        await page
          .getByTestId("mfa-recovery")
          .waitFor({ state: "visible", timeout: 5_000 });
        return true;
      } catch {
        return false;
      }
    };
    let enabled = await tryEnable();
    if (!enabled) {
      enabled = await tryEnable();
    }
    expect(enabled, "enable succeeded within two step attempts").toBe(true);
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

    await mp.getByTestId("mfa-login-code").fill(totpAt(secret, currentStep()));
    await mp.getByTestId("mfa-verify-submit").click();
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
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
    // The code must be from a NEW step: test 3 already consumed the current
    // step, and replay protection correctly rejects reuse (window 0).
    const nextStepCode = () => totpAt(secret, currentStep() + 1);
    await mp.getByTestId("mfa-login-code").fill(nextStepCode());
    await mp.getByTestId("mfa-verify-submit").click();
    try {
      await mp.getByRole("heading", { name: "My Tasks" }).waitFor({
        state: "visible",
        timeout: 10_000,
      });
    } catch {
      // The boundary has not passed yet — wait for it and retry once.
      const sleepMs = (currentStep() + 1) * 30_000 - Date.now() + 500;
      await mp.waitForTimeout(Math.max(sleepMs, 1000));
      await mp.getByTestId("mfa-login-code").fill(totpAt(secret, currentStep()));
      await mp.getByTestId("mfa-verify-submit").click();
    }
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
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
