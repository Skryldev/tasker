import { AxeBuilder } from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

/**
 * Phase 10 quality gates:
 *  - visual baselines for the four highest-impact surfaces, light + dark
 *    (1% pixel threshold, animations disabled for determinism);
 *  - axe-core scans with zero critical violations on five key pages.
 *
 * Requires a freshly bootstrapped server (see scripts/e2e-server.sh):
 *   admin@tasker.local / admin-pass-12345
 * First run records the baselines under e2e/__screenshots__/.
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };

// Shared fixtures created once by the visual describe's beforeAll and reused
// by the axe scans.
let teamId = "";
let projectId = "";
let taskId = "";

test.describe.serial("Visual baselines", () => {
  test.beforeAll(async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    const team = await page.request.post("/api/v1/teams", {
      data: { name: "Quality Team" },
    });
    teamId = (await team.json()).id as string;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: "Quality Project" },
    });
    projectId = (await project.json()).id as string;
    // A teammate as assignee: the workflow forbids assignee == reporter and
    // the reporter defaults to the creator.
    const memberEmail = `quality-${Date.now().toString(36)}@test.local`;
    await page.request.post(`/api/v1/teams/${teamId}/members`, {
      data: { email: memberEmail, role: "member", temp_password: "member-temp-pw-123" },
    });
    const members = await page.request.get(`/api/v1/teams/${teamId}/members`);
    const assigneeId = (
      (await members.json()).items as { user: { id: string; email: string }; role: string }[]
    ).find((m) => m.user.email === memberEmail)!.user.id;
    for (const [title, status, due] of [
      ["Design the landing hero", "todo", null],
      ["Wire up the API client", "in_progress", null],
      ["Ship the release notes", "backlog", "2026-01-01"],
    ] as const) {
      const created = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
        data: { title, assignee_id: assigneeId, ...(due ? { due_date: due } : {}) },
      });
      expect(created.status(), `create task "${title}"`).toBe(201);
      if (title === "Wire up the API client") taskId = ((await created.json()) as { id: string }).id;
      void status;
    }
    await context.close();
  });

  async function login(page: Page) {
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  }

  async function screenshotPair(
    page: Page,
    name: string,
    prepare: () => Promise<void>,
  ) {
    for (const theme of ["light", "dark"] as const) {
      await page.evaluate((t) => {
        localStorage.setItem("tasker-theme", t);
      }, theme);
      await page.goto("/");
      await prepare();
      await expect(page).toHaveScreenshot(`${name}-${theme}.png`, {
        animations: "disabled",
        maxDiffPixelRatio: 0.01,
      });
    }
  }

  test("login page", async ({ page }) => {
    await page.goto("/login");
    await page.evaluate(() => localStorage.removeItem("tasker-theme"));
    await expect(page).toHaveScreenshot("login-light.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
    await page.evaluate(() => {
      localStorage.setItem("tasker-theme", "dark");
    });
    await page.goto("/login");
    await expect(page).toHaveScreenshot("login-dark.png", {
      animations: "disabled",
      maxDiffPixelRatio: 0.01,
    });
    await page.evaluate(() => localStorage.removeItem("tasker-theme"));
  });

  test("task list", async ({ page }) => {
    await login(page);
    await screenshotPair(page, "task-list", async () => {
      await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    });
  });

  test("kanban board", async ({ page }) => {
    await login(page);
    await screenshotPair(page, "kanban", async () => {
      await page.goto(`/projects/${projectId}?team=${teamId}&view=board`);
      await expect(page.getByTestId("kanban-board")).toBeVisible();
    });
  });

  test("task detail panel", async ({ page }) => {
    await login(page);
    await screenshotPair(page, "task-detail", async () => {
      await page.goto(`/tasks/${taskId}?team=${teamId}`);
      await expect(page.getByTestId("task-detail")).toBeVisible();
    });
  });
});

test.describe("Accessibility (axe-core)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  });

  async function expectNoCritical(
    page: Page,
    url: string,
    label: string,
  ) {
    await page.goto(url);
    await page.waitForLoadState("networkidle");
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
      .analyze();
    const critical = results.violations.filter((v) => v.impact === "critical");
    for (const v of results.violations) {
      console.log(
        `[axe:${label}] ${v.impact}: ${v.id} — ${v.nodes.length} node(s): ${v.help} @ ${v.nodes
          .map((n) => n.target.join(" "))
          .join(", ")}`,
      );
    }
    expect(critical, `${label} critical violations`).toHaveLength(0);
  }

  test("login", async ({ page }: { page: Page }) => {
    // Scan the login screen without a session.
    await page.evaluate(() => localStorage.clear());
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
      .analyze();
    const critical = results.violations.filter((v) => v.impact === "critical");
    for (const v of results.violations) {
      console.log(`[axe:login] ${v.impact}: ${v.id} — ${v.nodes.length} node(s): ${v.help}`);
    }
    expect(critical).toHaveLength(0);
  });

  test("task list", async ({ page }) => {
    await expectNoCritical(page, "/", "task-list");
  });

  test("kanban", async ({ page }) => {
    // The board lives on the shared fixture team/project from the visual
    // describe's beforeAll (never assume teams.items[0] has a project).
    await expectNoCritical(
      page,
      `/projects/${projectId}?team=${teamId}&view=board`,
      "kanban",
    );
  });

  test("task detail", async ({ page }) => {
    await expectNoCritical(page, `/tasks/${taskId}?team=${teamId}`, "task-detail");
  });

  test("team settings", async ({ page }) => {
    const teams = await page.request.get("/api/v1/teams");
    const firstTeam = ((await teams.json()).items as { id: string }[])[0];
    await expectNoCritical(
      page,
      `/teams/${firstTeam.id}/settings?team=${firstTeam.id}`,
      "team-settings",
    );
  });
});
