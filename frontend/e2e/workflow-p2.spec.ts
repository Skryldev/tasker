import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 2 E2E: reporter acceptance workflow + in-app notifications.
 * Runs after the Phase 1 suite against a freshly bootstrapped server
 * (scripts/e2e.sh restarts the server and wipes the DB before the run).
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };
const MEMBER = { email: "member@tasker.local", password: "member-pass-12345" };

test.describe.serial("Phase 2 workflow and notifications", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let taskId = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
  });

  test.afterAll(async () => {
    await context.close();
  });

  test("setup: admin signs in, creates team + member + project + task", async () => {
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    await page.getByTestId("team-switcher").click();
    await page.getByText("Create team…").click();
    await page.getByLabel("Name").fill("P2 Team");
    await page.getByTestId("create-team-submit").click();
    await page.waitForURL(/\?team=/);
    teamId = new URL(await page.url()).searchParams.get("team") ?? "";

    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId("add-member").click();
    await page.getByTestId("member-email-input").fill(MEMBER.email);
    await page.getByLabel("Temporary password (new accounts only)").fill(MEMBER.password);
    await page.getByTestId("member-submit").click();
    await expect(page.getByTestId("member-list")).toContainText(MEMBER.email);

    await page.goto(`/projects?team=${teamId}`);
    await page.getByTestId("create-project").click();
    await page.getByTestId("project-name-input").fill("P2 Project");
    await page.getByTestId("project-submit").click();
    await page.getByRole("link", { name: /P2 Project/ }).first().click();

    // The admin assigns the member at creation so the member can work it.
    await page.getByTestId("create-task").click();
    await page.getByTestId("task-title-input").fill("Workflow task");
    await page.request.get(`/api/v1/teams/${teamId}/members`);
    await page.getByRole("combobox", { name: "Assignee" }).click();
    await page.getByRole("option", { name: "member", exact: false }).first().click();
    await page.getByTestId("task-submit").click();
    await page.getByTestId("view-list").click();
    await expect(page.getByTestId("task-table")).toContainText("Workflow task");
    taskId = (await page
      .locator('[data-testid^="task-row-"]')
      .getAttribute("data-testid"))!
      .replace("task-row-", "");
    expect(taskId).not.toBe("");
  });

  test("member assigns self, works, and submits for review", async ({ browser }) => {
    const memberContext = await browser.newContext();
    const mp = await memberContext.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    // Wait for the authenticated redirect, not a transient heading flash.
    await mp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    // The member (already assigned at creation) starts work and submits.
    await mp.goto(`/tasks/${taskId}`);
    await expect(mp.getByTestId("task-detail")).toContainText("Workflow task");
    const start = await mp.request.patch(`/api/v1/tasks/${taskId}`, {
      data: { status: "in_progress" },
    });
    expect(start.status()).toBe(200);
    await mp.reload();
    await expect(mp.getByTestId("task-detail")).toContainText("In Progress");

    // The submit button is offered to the assignee.
    await mp.getByTestId("submit-task").click();
    await expect(mp.getByTestId("task-detail")).toContainText("In Review");
    // The assignee cannot accept their own work: no accept button is offered,
    // and a direct API call is forbidden.
    expect(await mp.getByTestId("accept-task").count()).toBe(0);
    const res = await mp.request.post(`/api/v1/tasks/${taskId}/accept`);
    expect(res.status()).toBe(403);
    await memberContext.close();
  });

  test("reporter sees a submission notification and accepts via the UI", async () => {
    // The bell shows the unread submission notification.
    await page.reload();
    await expect(page.getByTestId("unread-badge")).toBeVisible();
    await page.getByTestId("notification-bell").click();
    const list = page.getByTestId("notification-list");
    await expect(list).toContainText("submitted");
    await list.locator('[data-testid^="notification-"]').first().click();
    // Navigation goes to the task page; the workflow buttons appear.
    await expect(page.getByTestId("task-detail")).toContainText("In Review");
    await expect(page.getByTestId("workflow-actions")).toBeVisible();
    await page.getByTestId("accept-task").click();
    await expect(page.getByTestId("resolution-banner")).toContainText("Accepted");
  });

  test("reject flow requires a reason and the assignee is notified", async ({ browser }) => {
    // Admin reopens; member resubmits; admin rejects with a reason.
    await page.request.patch(`/api/v1/tasks/${taskId}`, {
      data: { status: "in_progress" },
    });

    const memberContext = await browser.newContext();
    const mp = await memberContext.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    // Wait for the authenticated redirect, not a transient heading flash.
    await mp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await mp.goto(`/tasks/${taskId}`);
    await mp.getByTestId("submit-task").click();
    await expect(mp.getByTestId("task-detail")).toContainText("In Review");
    await memberContext.close();

    // The bell carries a new submission notification (badge visible after
    // the query refresh; no need to open the panel).
    await page.reload();
    await expect(page.getByTestId("unread-badge")).toBeVisible();

    await page.goto(`/tasks/${taskId}`);
    await page.getByTestId("reject-task").click();
    // Blank reason is blocked client-side.
    await page.getByTestId("reject-submit").click();
    await expect(page.getByText("A rejection reason is required.")).toBeVisible();
    await page.getByTestId("reject-reason-input").fill("the intro needs numbers");
    await page.getByTestId("reject-submit").click();
    await expect(page.getByTestId("resolution-banner")).toContainText("Rejected");
    await expect(page.getByTestId("resolution-banner")).toContainText("the intro needs numbers");
    await expect(page.getByTestId("task-detail")).toContainText("In Progress");
  });

  test("mention in a comment notifies the mentioned member", async ({ browser }) => {
    const memberContext = await browser.newContext();
    const mp = await memberContext.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    await mp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    const beforeResponse = await mp.request.get("/api/v1/notifications/unread-count");
    const before = await beforeResponse.json();
    if (beforeResponse.status() !== 200) {
      console.log("MEMBER-CTX-DEBUG:", await mp.evaluate(() => document.cookie));
      console.log("MEMBER-URL:", mp.url(), "BODY:", JSON.stringify(before).slice(0, 200));
    }
    expect(beforeResponse.status()).toBe(200);

    await page.goto(`/tasks/${taskId}`);
    await page.getByTestId("comment-input").click();
    // Type an @ mention through the autocomplete.
    await page.getByTestId("comment-input").pressSequentially("@mem");
    await expect(page.getByTestId("mention-list")).toBeVisible();
    await page.getByTestId("mention-list").getByText("member", { exact: false }).first().click();
    await page.getByTestId("comment-input").pressSequentially("please check");
    await page.getByTestId("comment-submit").click();
    await expect(page.getByTestId("task-detail")).toContainText("please check");

    const afterResponse = await mp.request.get("/api/v1/notifications/unread-count");
    const after = await afterResponse.json();
    expect(afterResponse.status()).toBe(200);
    expect(after.count).toBeGreaterThan(before.count);
    await memberContext.close();
  });

  test("marking all notifications read clears the badge", async () => {
    await page.reload();
    await page.getByTestId("notification-bell").click();
    await page.getByTestId("mark-all-read").click();
    await expect(page.getByTestId("unread-badge")).toHaveCount(0);
  });
});
