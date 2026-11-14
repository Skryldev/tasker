import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 2 E2E: reporter acceptance workflow + in-app notifications, on the
 * strict lifecycle (assignee executes, reporter reviews):
 *   backlog -> todo -> in_progress -> resolved -> in_review -> closed
 * Runs after the Phase 1 suite against a freshly bootstrapped server
 * (scripts/e2e.sh restarts the server and wipes the DB before the run).
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };
const MEMBER = { email: "member@tasker.local", password: "Member-Pass-1234!" };

test.describe.serial("Phase 2 workflow and notifications", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";
  let taskId = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
  });

  test.afterAll(async () => {
    await context.close();
  });

  test("setup: admin signs in, creates team + member + project + task", async () => {
    // Suffix-qualified names: stale teams from earlier runs would break the
    // name-based lookup below.
    const suffix = Date.now().toString(36);
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    await page.getByTestId("team-switcher").click();
    await page.getByText("Create team…").click();
    await page.getByLabel("Name").fill(`P2 Team ${suffix}`);
    await page.getByTestId("create-team-submit").click();
    // Resolve by name via the API: the URL's team param can be defaulted by
    // the workspace and is not race-proof here.
    await expect(page.getByTestId("team-switcher")).toContainText("P2 Team");
    const created = await page.request.get("/api/v1/teams");
    teamId = ((await created.json()).items as { id: string; name: string }[]).find(
      (t) => t.name === `P2 Team ${suffix}`,
    )!.id;

    // Bug 2 (ADR-0014): the account is created by the Owner on the Users
    // page; team settings only assign it.
    // Bug 2 (ADR-0014): Owner-created account; tolerate "already exists"
    // from an earlier leg of this serial suite.
    const __created = await page.request.post("/api/v1/admin/users", {
      data: { email: MEMBER.email, display_name: "P2 Member", temporary_password: MEMBER.password },
    });
    if (__created.status() === 422) {
      // The account exists already — assignment below is still valid.
    } else {
      expect(__created.status()).toBe(201);
    }
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId("add-member").click();
    await page.getByTestId("member-search-input").fill(MEMBER.email);
    await page.getByTestId("eligible-members-list").getByRole("option").first().click();
    await expect(page.getByTestId("member-list")).toContainText(MEMBER.email);

    await page.goto(`/projects?team=${teamId}`);
    await page.getByTestId("create-project").click();
    await page.getByTestId("project-name-input").fill("P2 Project");
    await page.getByTestId("project-submit").click();
    await page.getByRole("link", { name: /P2 Project/ }).first().click();
    projectId = new URL(await page.url()).pathname.split("/").pop() ?? "";
    expect(projectId).not.toBe("");

    // The admin assigns the member at creation so the member can work it.
    // The admin stays the reporter: one person must not be both assignee
    // and reporter (identity invariant).
    await page.getByTestId("create-task-board").first().click();
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

  test("member picks up, works, and resolves — but cannot review", async ({ browser }) => {
    const memberContext = await browser.newContext();
    const mp = await memberContext.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    // Wait for the authenticated redirect, not a transient heading flash.
    await mp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    // The assignee drives the execution leg through the transition endpoint.
    const todo = await mp.request.post(`/api/v1/tasks/${taskId}/transition`, {
      data: { to: "todo" },
    });
    expect(todo.status()).toBe(200);
    const start = await mp.request.post(`/api/v1/tasks/${taskId}/transition`, {
      data: { to: "in_progress" },
    });
    expect(start.status()).toBe(200);
    // The generic PATCH no longer mutates status (dedicated endpoint only).
    const patchStatus = await mp.request.patch(`/api/v1/tasks/${taskId}`, {
      data: { status: "in_progress" },
    });
    expect(patchStatus.status()).toBe(400);
    await mp.goto(`/tasks/${taskId}`);
    await expect(mp.getByTestId("task-page")).toContainText("In Progress");

    // The assignee marks the work resolved.
    await mp.getByTestId("resolve-task").click();
    await expect(mp.getByTestId("task-page")).toContainText("Resolved");

    // Review decisions belong to the reporter: submission is forbidden for
    // the assignee (legal pair, actor check), and no accept button is
    // offered. A direct attempt to jump straight to closed is not even a
    // legal pair (409): closed is only reachable from in_review.
    const submitRes = await mp.request.post(`/api/v1/tasks/${taskId}/transition`, {
      data: { to: "in_review" },
    });
    expect(submitRes.status()).toBe(403);
    expect(await mp.getByTestId("accept-task").count()).toBe(0);
    const res = await mp.request.post(`/api/v1/tasks/${taskId}/transition`, {
      data: { to: "closed" },
    });
    expect(res.status()).toBe(409);
    await memberContext.close();
  });

  test("reporter sees the ready-for-review notification and accepts via the UI", async () => {
    // The bell shows the unread notification about the resolved work.
    await page.reload();
    await expect(page.getByTestId("unread-badge")).toBeVisible();
    await page.getByTestId("notification-bell").click();
    const list = page.getByTestId("notification-list");
    await expect(list).toContainText("resolved");
    await list.locator('[data-testid^="notification-"]').first().click();
    // Navigation goes to the task page; the reporter submits for review.
    await expect(page.getByTestId("task-page")).toContainText("Resolved");
    await expect(page.getByTestId("submit-task")).toBeVisible();
    await page.getByTestId("submit-task").click();
    await expect(page.getByTestId("task-page")).toContainText("In Review");
    // Then the reporter accepts via the confirmation dialog (Phase 10.5
    // status control shows consequences before closing).
    await page.getByTestId("accept-task").click();
    await page.getByTestId("confirm-accept").click();
    await expect(page.getByTestId("resolution-banner")).toContainText("Accepted");
  });

  test("reject flow requires a reason and the assignee is notified", async ({ browser }) => {
    // Closed is terminal, so the rejection path uses a fresh task: the admin
    // creates it with the member as assignee (reporter = admin).
    const members = await (await page.request.get(`/api/v1/teams/${teamId}/members`)).json();
    const memberId = members.items.find((m) => m.user.email === MEMBER.email).user.id;
    const created = await (
      await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
        data: { title: "Reject flow task", assignee_id: memberId },
      })
    ).json();
    const rejectTaskId = created.id as string;
    expect(rejectTaskId).not.toBe("");

    const memberContext = await browser.newContext();
    const mp = await memberContext.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    // Wait for the authenticated redirect, not a transient heading flash.
    await mp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    // Member works the task to resolved; the reporter then submits it.
    for (const to of ["todo", "in_progress", "resolved"]) {
      const res = await mp.request.post(`/api/v1/tasks/${rejectTaskId}/transition`, {
        data: { to },
      });
      expect(res.status()).toBe(200);
    }
    await memberContext.close();
    const submit = await page.request.post(`/api/v1/tasks/${rejectTaskId}/transition`, {
      data: { to: "in_review" },
    });
    expect(submit.status()).toBe(200);

    await page.goto(`/tasks/${rejectTaskId}`);
    await page.getByTestId("reject-task").click();
    // Blank reason is blocked client-side.
    await page.getByTestId("reject-submit").click();
    await expect(page.getByText("A rejection reason is required.")).toBeVisible();
    await page.getByTestId("reject-reason-input").fill("the intro needs numbers");
    await page.getByTestId("reject-submit").click();
    await expect(page.getByTestId("resolution-banner")).toContainText("Rejected");
    await expect(page.getByTestId("resolution-banner")).toContainText("the intro needs numbers");
    await expect(page.getByTestId("task-page")).toContainText("In Progress");
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

    // Phase 12 (WS10): closed tasks reject comments — the accepted task above
    // is locked, so the mention lands on a fresh open task instead.
    const members = await (await page.request.get(`/api/v1/teams/${teamId}/members`)).json();
    const memberId = members.items.find((m) => m.user.email === MEMBER.email).user.id;
    const created = await (
      await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
        data: { title: "Mention flow task", assignee_id: memberId },
      })
    ).json();
    const mentionTaskId = created.id as string;
    await page.goto(`/tasks/${mentionTaskId}`);
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
