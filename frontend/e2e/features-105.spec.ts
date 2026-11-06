import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 10.5 feature journeys: display-name fix, status control, project
 * delete (undo / restore / purge), and the capacity view.
 *
 * Requires a freshly bootstrapped server (scripts/e2e-server.sh):
 *   admin@tasker.local / admin-pass-12345
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };
const MEMBER = { email: "member105@tasker.local", password: "member-pass-12345" };

test.describe.serial("Phase 10.5 features", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";
  let taskId = "";
  let memberId = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await login(page);

    const suffix = Date.now().toString(36);
    const team = await page.request.post("/api/v1/teams", {
      data: { name: `P105 Team ${suffix}` },
    });
    teamId = (await team.json()).id as string;
    await page.request.post(`/api/v1/teams/${teamId}/members`, {
      data: { email: MEMBER.email, role: "member", temp_password: MEMBER.password },
    });
    const members = await page.request.get(`/api/v1/teams/${teamId}/members`);
    memberId = (
      (await members.json()).items as { user: { id: string; email: string }; role: string }[]
    ).find((m) => m.user.email === MEMBER.email)!.user.id;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `P105 Project ${suffix}` },
    });
    projectId = (await project.json()).id as string;
    // Assignee = member, reporter = admin (workflow requires distinct users).
    const task = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P105 seed task", assignee_id: memberId },
    });
    taskId = ((await task.json()) as { id: string }).id;
    // Two more tasks for capacity numbers.
    await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P105 second", assignee_id: memberId, due_date: "2020-01-01" },
    });
  });

  test.afterAll(async () => {
    await context.close();
  });

  async function login(page: Page) {
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  }

  test("reporter select shows the display name, not 'Me'", async () => {
    await page.goto(`/projects/${projectId}?team=${teamId}&new=task`);
    await page.getByLabel("Reporter").click();
    await expect(
      page.getByRole("option", { name: /Ada Admin \(you\)/ }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
  });

  test("status control: current status, allowed menu, reject reason, accept confirm", async ({ browser }) => {
    await page.goto(`/tasks/${taskId}?team=${teamId}`);
    await expect(page.getByTestId("status-control")).toBeVisible();
    await expect(page.getByTestId("status-control")).toHaveAttribute("data-status", "backlog");

    // Admin is the reporter (not the assignee): the assignee-only "Move to
    // Todo" is disabled in the menu with an explanatory tooltip.
    await page.getByTestId("status-control-trigger").click();
    const todoItem = page.getByTestId("transition-todo");
    await expect(todoItem).toBeDisabled();
    await expect(todoItem).toHaveAttribute("title", /Only the assignee/);
    await page.keyboard.press("Escape");

    // Drive: assignee picks up → in progress → resolved; reporter submits →
    // in review; reporter rejects with a reason → back to in progress.
    // A separate context: the member's session cookie must not clobber the
    // admin's cookie in the shared jar.
    const memberContext = await browser.newContext();
    const memberPage = await memberContext.newPage();
    await memberPage.goto("/login");
    await memberPage.getByLabel("Email").fill(MEMBER.email);
    await memberPage.getByLabel("Password").fill(MEMBER.password);
    await memberPage.getByTestId("login-submit").click();
    await expect(memberPage.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await memberPage.goto(`/tasks/${taskId}?team=${teamId}`);
    await memberPage.getByTestId("start-task").click();
    await expect(memberPage.getByTestId("status-control")).toHaveAttribute("data-status", "todo");
    await memberPage.getByTestId("start-task").click();
    await expect(memberPage.getByTestId("status-control")).toHaveAttribute("data-status", "in_progress");
    await memberPage.getByTestId("resolve-task").click();
    await expect(memberPage.getByTestId("status-control")).toHaveAttribute("data-status", "resolved");
    await memberPage.close();

    await page.goto(`/tasks/${taskId}?team=${teamId}`);
    await page.getByTestId("submit-task").click();
    await expect(page.getByTestId("status-control")).toHaveAttribute("data-status", "in_review");

    // Reject requires a reason (client-side guard first).
    await page.getByTestId("reject-task").click();
    await page.getByTestId("reject-submit").click();
    await expect(page.getByText("A rejection reason is required.")).toBeVisible();
    await page.getByTestId("reject-reason-input").fill("needs numbers");
    await page.getByTestId("reject-submit").click();
    await expect(page.getByTestId("status-control")).toHaveAttribute("data-status", "in_progress");

    // Member resolves again; admin accepts through the confirmation dialog.
    const memberContext2 = await browser.newContext();
    const memberPage2 = await memberContext2.newPage();
    await memberPage2.goto("/login");
    await memberPage2.getByLabel("Email").fill(MEMBER.email);
    await memberPage2.getByLabel("Password").fill(MEMBER.password);
    await memberPage2.getByTestId("login-submit").click();
    // Wait for the session before navigating; an immediate goto races the
    // login POST and drops the cookie.
    await expect(memberPage2.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await memberPage2.goto(`/tasks/${taskId}?team=${teamId}`);
    // The rejection returned the task to in_progress, so resolve directly.
    await memberPage2.getByTestId("resolve-task").click();
    await memberContext2.close();

    await page.goto(`/tasks/${taskId}?team=${teamId}`);
    await expect(page.getByTestId("status-control")).toHaveAttribute("data-status", "resolved");
    await page.getByTestId("submit-task").click();
    await page.getByTestId("accept-task").click();
    await expect(page.getByTestId("confirm-accept")).toBeVisible();
    await page.getByTestId("confirm-accept").click();
    await expect(page.getByTestId("status-control")).toHaveAttribute("data-status", "closed");
  });

  test("project delete → undo → purge from trash", async () => {
    // Create a dedicated project for the delete journey.
    const suffix = Date.now().toString(36);
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `Delete Me ${suffix}` },
    });
    const delProjectId = (await project.json()).id as string;
    await page.goto(`/projects?team=${teamId}`);
    await expect(page.getByTestId("project-list")).toContainText(`Delete Me ${suffix}`);

    // Delete from the card menu.
    await page.getByTestId(`project-menu-${delProjectId}`).click();
    await page.getByTestId("delete-project").click();
    await page.getByRole("button", { name: "Delete project" }).click();
    await expect(page.getByTestId("project-list")).not.toContainText(`Delete Me ${suffix}`);

    // Undo within the toast's 5 s window restores it.
    await page.getByRole("button", { name: "Undo" }).click();
    await expect(page.getByTestId("project-list")).toContainText(`Delete Me ${suffix}`, {
      timeout: 10_000,
    });

    // Soft-delete again; the team trash lists it; purge with typed name +
    // password removes it permanently.
    await page.getByTestId(`project-menu-${delProjectId}`).click();
    await page.getByTestId("delete-project").click();
    await page.getByRole("button", { name: "Delete project" }).click();
    await expect(page.getByTestId("project-list")).not.toContainText(`Delete Me ${suffix}`);

    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    const trashList = page.getByTestId("trash-list");
    await expect(trashList).toBeVisible();
    await trashList.getByTestId(`restore-project-${delProjectId}`).waitFor({ state: "visible" });
    // First restore via trash, then delete once more and purge.
    await trashList.getByTestId(`restore-project-${delProjectId}`).click();
    await expect(page.getByTestId("trash-list")).toHaveCount(0);
    await page.goto(`/projects?team=${teamId}`);
    await page.getByTestId(`project-menu-${delProjectId}`).click();
    await page.getByTestId("delete-project").click();
    await page.getByRole("button", { name: "Delete project" }).click();

    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId(`purge-project-${delProjectId}`).click();
    await page.getByTestId("purge-project-name").fill(`Delete Me ${suffix}`);
    await page.getByTestId("purge-password").fill(ADMIN.password);
    await page.getByTestId("purge-submit").click();
    await expect(page.getByTestId("trash-empty")).toBeVisible();
    const check = await page.request.get(`/api/v1/projects/${delProjectId}`);
    expect(check.status()).toBe(404);
  });

  test("capacity view: load, window switch, sort", async () => {
    await page.goto(`/teams/${teamId}/capacity?team=${teamId}`);
    await expect(page.getByTestId("capacity-summary")).toBeVisible();
    // Two open tasks were seeded for the member.
    const table = page.getByTestId("capacity-table");
    await expect(table).toBeVisible();
    const memberRow = table.getByTestId(`capacity-row-${memberId}`);
    await expect(memberRow).toBeVisible();
    // The overdue seeded task shows in red.
    await expect(memberRow).toContainText("1");

    // Switch the window to month.
    await page.getByLabel("Capacity window").click();
    await page.getByRole("option", { name: "This month" }).click();
    await expect(page.getByLabel("Capacity window")).toContainText("This month");
    await expect(page.getByTestId("capacity-table")).toBeVisible();

    // Sort by name keeps the table rendered.
    await page.getByLabel("Sort members").click();
    await page.getByRole("option", { name: "Sort by name" }).click();
    await expect(page.getByTestId("capacity-table")).toBeVisible();
  });
});
