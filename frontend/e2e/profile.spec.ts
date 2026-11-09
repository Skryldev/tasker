import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Profile journeys (Phase 9): work identity editing, notification preference
 * persistence, the My Work Summary, and the active-sessions list.
 *
 * Requires a freshly bootstrapped server (see scripts/e2e-server.sh):
 *   admin@tasker.local / Admin-Pass-12345!
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };
const USERNAME = `e2e_${Date.now().toString(36)}`.slice(0, 32);

test.describe.serial("Profile", () => {
  let context: BrowserContext;
  let page: Page;

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
  });

  test.afterAll(async () => {
    await context.close();
  });

  test.beforeEach(async () => {
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  });

  test.afterEach(async () => {
    // Sign out via the UI to keep each journey isolated.
    await page.getByTestId("user-menu").click();
    await page.getByTestId("logout").click();
    await page.waitForURL("**/login");
  });

  test("edits work identity: username, bio, timezone", async () => {
    await page.getByTestId("user-menu").click();
    await page.getByRole("menuitem", { name: "Settings" }).click();
    await expect(page.getByRole("heading", { name: "Profile" })).toBeVisible();

    const usernameInput = page.getByTestId("username-input");
    await usernameInput.fill("Bad Name!");
    await page.getByTestId("profile-save").click();
    await expect(page.getByText("Username must be 3-32 characters")).toBeVisible();

    await usernameInput.fill(USERNAME);
    await page.getByTestId("bio-input").fill("E2E profile bio");
    await page.getByRole("combobox").first().click();
    await page.getByRole("option", { name: "Europe/Berlin" }).click();
    await page.getByTestId("profile-save").click();

    // The saved values survive a reload (server persistence).
    await page.reload();
    await expect(page.getByTestId("username-input")).toHaveValue(USERNAME);
    await expect(page.getByTestId("bio-input")).toHaveValue("E2E profile bio");
  });

  test("toggles a notification preference and persists it", async () => {
    await page.goto("/account");
    const toggle = page.getByTestId("pref-task-rejected");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    // Auto-save debounces 300ms — wait for the Saved flash before reloading.
    await expect(page.getByTestId("prefs-saved")).toHaveAttribute("style", /opacity: 1/);

    await page.reload();
    await expect(page.getByTestId("pref-task-rejected")).toHaveAttribute(
      "aria-checked",
      "false",
    );
    // Restore the default for later journeys.
    await page.getByTestId("pref-task-rejected").click();
    await expect(page.getByTestId("pref-task-rejected")).toHaveAttribute("aria-checked", "true");
  });

  test("shows the My Work Summary with server counts and links", async () => {
    await page.goto("/account");
    const summary = page.getByTestId("my-work-summary");
    await expect(summary).toBeVisible();
    // Six lifecycle tiles are rendered.
    for (const status of ["backlog", "todo", "in-progress", "resolved", "in-review", "closed"]) {
      await expect(summary.getByTestId(`summary-${status}`)).toBeVisible();
    }
    await expect(page.getByTestId("summary-overdue")).toBeVisible();
    await expect(page.getByTestId("summary-mentions")).toBeVisible();

    // A tile navigates to the filtered task list with the URL filter applied.
    await page.getByTestId("summary-todo").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await expect(page).toHaveURL(/status=todo/);
  });

  test("lists active sessions and revokes a session", async ({ browser }) => {
    // A second browser context simulates a second device (its own cookie
    // jar, therefore its own session).
    const secondContext = await browser.newContext();
    const secondPage = await secondContext.newPage();
    await secondPage.goto("/login");
    await secondPage.getByLabel("Email").fill(ADMIN.email);
    await secondPage.getByLabel("Password").fill(ADMIN.password);
    await secondPage.getByTestId("login-submit").click();
    await expect(secondPage.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    await page.goto("/account");
    const sessionList = page.getByTestId("session-list");
    await expect(sessionList).toBeVisible();
    await expect(sessionList).toContainText("This device");

    // Revoke every OTHER session. The exact count is not known up front: the
    // seed scripts log the admin in many times, so the list starts with
    // dozens of sessions. Synchronize each revocation on its DELETE response
    // and on the list refetch that invalidation triggers — a count() read
    // between click and refetch observes a stale render, which is what made
    // the previous while-loop's toHaveCount target race.
    const others = sessionList.getByTestId("revoke-session-other");
    for (;;) {
      const remaining = await others.count();
      if (remaining === 0) break;
      const revoked = page.waitForResponse(
        (r) => r.request().method() === "DELETE" && r.url().includes("/me/sessions/"),
      );
      const refetched = page.waitForResponse(
        (r) => r.request().method() === "GET" && r.url().endsWith("/me/sessions"),
      );
      await others.first().click();
      await revoked;
      await refetched;
      await expect(others).toHaveCount(remaining - 1);
    }

    // This tab stays signed in; the revoked device discovers the loss on its
    // next query and lands on /login.
    await expect(page.getByRole("heading", { name: "Profile" })).toBeVisible();
    await secondPage.getByRole("link", { name: "Projects" }).click();
    await secondPage.waitForURL("**/login");
    await expect(secondPage.getByTestId("login-form")).toBeVisible();
    await secondContext.close();
  });
});
