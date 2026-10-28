import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * End-to-end workflow against the real backend:
 * bootstrap -> login -> create team/project/task -> assign -> move status
 * -> reload -> verify persistence, plus negative authorization checks.
 *
 * Requires a freshly bootstrapped server (see scripts/e2e-server.sh):
 *   admin@tasker.local / admin-pass-12345
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };
const MEMBER = { email: "member@tasker.local", password: "member-pass-12345" };

test.describe.serial("Tasker core workflow", () => {
  let teamId = "";
  let projectId = "";
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

  test("login page renders and rejects wrong credentials", async () => {
    await page.goto("/login");
    await expect(page.getByRole("heading", { name: "Sign in to Tasker" })).toBeVisible();
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill("totally-wrong-password");
    await page.getByTestId("login-submit").click();
    await expect(page.getByTestId("login-error")).toContainText("Incorrect email or password");
  });

  test("administrator signs in", async () => {
    await loginAs(ADMIN.email, ADMIN.password);
  });

  test("create a team", async () => {
    await page.goto("/");
    await page.getByTestId("team-switcher").click();
    await page.getByText("Create team…").click();
    await page.getByLabel("Name").fill("E2E Team");
    await page.getByTestId("create-team-submit").click();
    await expect(page.getByTestId("team-switcher")).toContainText("E2E Team");
    const url = new URL(page.url());
    teamId = url.searchParams.get("team") ?? "";
    expect(teamId).not.toBe("");
  });

  test("add a member with a temporary password", async () => {
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId("add-member").click();
    await page.getByTestId("member-email-input").fill(MEMBER.email);
    await page.getByLabel("Role").click();
    await page.getByRole("option", { name: "Member" }).click();
    await page.getByLabel("Temporary password (new accounts only)").fill(MEMBER.password);
    await page.getByTestId("member-submit").click();
    await expect(page.getByTestId("member-list")).toContainText(MEMBER.email);
  });

  test("member cannot change roles (negative authorization)", async ({ browser }) => {
    // Sign in as the member in a separate context.
    const memberPage = await browser.newContext();
    const mp = await memberPage.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    // Member opens team settings: no role controls may be offered.
    await mp.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await expect(mp.getByTestId("add-member")).toHaveCount(0);
    // Direct API escalation attempt is forbidden by the backend.
    const res = await mp.request.post(`/api/v1/teams/${teamId}/members`, {
      data: { email: "someone-else@tasker.local", role: "owner" },
    });
    expect(res.status()).toBe(403);
    await memberPage.close();
  });

  test("create a project and a task, assign, and move it across the board", async () => {
    await page.goto(`/projects?team=${teamId}`);
    await page.getByTestId("create-project").click();
    await page.getByTestId("project-name-input").fill("Launch Plan");
    await page.getByTestId("project-submit").click();
    await expect(page.getByTestId("project-list")).toContainText("Launch Plan");

    await page.getByRole("link", { name: /Launch Plan/ }).click();
    await expect(page.getByRole("heading", { name: "Launch Plan" })).toBeVisible();
    const url = new URL(page.url());
    projectId = url.pathname.split("/").pop() ?? "";
    expect(projectId).not.toBe("");

    // Create a task from the board's Todo column.
    await page.getByTestId("view-board").click();
    await page.getByTestId("create-task-board").click();
    await page.getByTestId("task-title-input").fill("Write announcement post");
    await page.getByLabel("Status").click();
    await page.getByRole("option", { name: "Todo" }).click();
    await page.getByTestId("task-submit").click();
    const card = page.locator('[data-testid^="kanban-card-"]', { hasText: "Write announcement post" });
    await expect(card).toBeVisible();

    // Move the card to In Progress via drag and drop.
    const cardId = (await card.getAttribute("data-testid"))!.replace("kanban-card-", "");
    await card.hover();
    await page.mouse.down();
    const target = page.getByTestId("kanban-column-in_progress");
    const box = await target.boundingBox();
    await page.mouse.move(box!.x + box!.width / 2, box!.y + 40, { steps: 8 });
    await page.mouse.up();
    await expect(page.getByTestId("kanban-column-in_progress")).toContainText(
      "Write announcement post",
    );

    // The change persisted to the backend.
    const list = await page.request.get(
      `/api/v1/projects/${projectId}/tasks?status=in_progress`,
    );
    const body = await list.json();
    expect(body.total).toBe(1);
    expect(body.items[0].title).toBe("Write announcement post");
    expect(body.items[0].id).toBe(cardId);
  });

  test("changes survive a full page reload", async () => {
    await page.goto(`/projects/${projectId}?view=board`);
    await expect(page.getByTestId("kanban-column-in_progress")).toContainText(
      "Write announcement post",
    );
    // List view reflects the same persisted state with filters.
    await page.getByTestId("view-list").click();
    await page.getByLabel("Filter by status").click();
    await page.getByRole("option", { name: "In Progress" }).click();
    await expect(page.getByTestId("task-table")).toContainText("Write announcement post");
  });

  test("logout revokes the session", async () => {
    await page.getByTestId("user-menu").click();
    await page.getByTestId("logout").click();
    await expect(page).toHaveURL(/\/login$/);
    // The revoked cookie no longer grants access.
    const res = await page.request.get("/api/v1/me");
    expect(res.status()).toBe(401);
  });
});
