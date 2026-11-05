import { expect, test, type BrowserContext, type Page, type Request } from "@playwright/test";

/**
 * Logout bug regression + global 401 handling (Phase 9 blocker).
 *
 * Journeys:
 * 1. login -> logout -> immediate redirect to /login without a manual refresh,
 *    with no "Try again" error state rendering.
 * 2. dead session (expired/revoked) -> next query 401 -> redirect to /login,
 *    again without any "Try again" state.
 * 3. logout produces no redirect loop and no refetch storm.
 * 4. logout -> login -> logout still works (signed-out state re-arms).
 *
 * Requires a freshly bootstrapped server (see scripts/e2e-server.sh):
 *   admin@tasker.local / admin-pass-12345
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };

test.describe.serial("Logout and global 401 handling", () => {
  let context: BrowserContext;
  let page: Page;

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
  });

  test.afterAll(async () => {
    await context.close();
  });

  async function loginAs(email: string, password: string) {
    await page.goto("/login");
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill(password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  }

  /** Count exact `/api/v1/me` requests while the returned disposer is active. */
  function trackMeRequests(): () => number {
    let count = 0;
    const listener = (req: Request) => {
      if (new URL(req.url()).pathname === "/api/v1/me") count += 1;
    };
    page.on("request", listener);
    return () => count;
  }

  test("logout redirects immediately to /login without a refresh", async () => {
    await loginAs(ADMIN.email, ADMIN.password);

    const meCount = trackMeRequests();
    await page.getByTestId("user-menu").click();
    await page.getByTestId("logout").click();

    // Redirect happens on the mutation success — no manual reload.
    await page.waitForURL("**/login");
    await expect(page.getByTestId("login-form")).toBeVisible();

    // No error state ("Try again") may render for the session teardown.
    await expect(page.getByTestId("error-state")).toHaveCount(0);

    // No redirect loop, no refetch storm: the URL stays on /login and /me
    // receives at most the single post-clear refetch (401, unretried).
    await page.waitForTimeout(2000);
    expect(page.url()).toContain("/login");
    expect(meCount()).toBeLessThanOrEqual(3);
    await expect(page.getByTestId("login-form")).toBeVisible();
  });

  test("expired session: next query 401 redirects to /login with no Try again", async () => {
    await loginAs(ADMIN.email, ADMIN.password);

    // A team is required for the Projects sidebar link to render. Created
    // through the authenticated API, then a reload so the workspace picks
    // it up (the teams query has no cross-tab invalidation).
    const created = await page.request.post("/api/v1/teams", {
      data: { name: "Expiry Team" },
    });
    if (created.status() === 201) {
      await page.reload();
      await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    }

    // Simulate server-side session expiry: every subsequent request is
    // unauthenticated, while the app still holds its cached user.
    await context.clearCookies();

    // The next query the app sends must trigger the global 401 handler.
    await page.getByRole("link", { name: "Projects" }).click();

    await page.waitForURL("**/login");
    await expect(page.getByTestId("login-form")).toBeVisible();

    // The projects page never gets to show its "Try again" error state —
    // the 401 must redirect instead.
    await expect(page.getByTestId("error-state")).toHaveCount(0);

    // Settle: still on /login, no loop.
    await page.waitForTimeout(2000);
    expect(page.url()).toContain("/login");
    await expect(page.getByTestId("login-form")).toBeVisible();
  });

  test("logout works again after signing back in (no stuck signed-out state)", async () => {
    // Re-login establishes a fresh session; the signed-out flag must be
    // cleared so this second logout also redirects immediately.
    await loginAs(ADMIN.email, ADMIN.password);

    await page.getByTestId("user-menu").click();
    await page.getByTestId("logout").click();
    await page.waitForURL("**/login");
    await expect(page.getByTestId("login-form")).toBeVisible();

    await page.waitForTimeout(1500);
    expect(page.url()).toContain("/login");
    await expect(page.getByTestId("login-form")).toBeVisible();
  });
});
