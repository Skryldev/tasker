import { AxeBuilder } from "@axe-core/playwright";
import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 12 E2E: 6-level priority + deadline, checklists + progress,
 * attachments, closed-task lock, label icons, theme groups, detail sidebar
 * toggle, notifications tabs, dashboard KPIs.
 *
 * Requires a freshly bootstrapped server (scripts/e2e-server.sh, which also
 * enables `TASKER_ATTACHMENTS=local`):
 *   admin@tasker.local / Admin-Pass-12345!
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };
const MEMBER = { email: "member12@tasker.local", password: "Member-Pass-1234!" };

test.describe.serial("Phase 12 features", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";
  let taskId = "";
  let memberId = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    const suffix = Date.now().toString(36);
    const team = await page.request.post("/api/v1/teams", {
      data: { name: `P12 Team ${suffix}` },
    });
    teamId = (await team.json()).id as string;
      // Bug 2 (ADR-0014): Owner-created account; tolerate "already exists"
      // from an earlier leg of this serial suite.
      const __created = await page.request.post("/api/v1/admin/users", {
        data: { email: MEMBER.email, display_name: "E2E Member", temporary_password: "Member-Pass-1234!" },
      });
      if (__created.status() === 422) {
        // The account exists already — assignment below is still valid.
      } else {
        expect(__created.status()).toBe(201);
      }
    await page.request.post(`/api/v1/teams/${teamId}/members`, {
      data: { email: MEMBER.email, role: "member" },
    });
    const members = await page.request.get(`/api/v1/teams/${teamId}/members`);
    memberId = (
      (await members.json()).items as { user: { id: string; email: string }; role: string }[]
    ).find((m) => m.user.email === MEMBER.email)!.user.id;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `P12 Project ${suffix}` },
    });
    projectId = (await project.json()).id as string;
    const task = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P12 seed", assignee_id: memberId, priority: "critical", deadline: "2027-01-01" },
    });
    taskId = ((await task.json()) as { id: string }).id;
    // A label with a whitelisted icon (WS5).
    await page.request.post(`/api/v1/teams/${teamId}/labels`, {
      data: { name: `Bug ${suffix}`, color: "#ef4444", icon: "bug" },
    });
  });

  test.afterAll(async () => {
    await context.close();
  });

  const boardUrl = () => `/projects/${projectId}?team=${teamId}&view=board`;

  test("priority picker offers six levels; critical task creates", async () => {
    await page.goto(boardUrl());
    await page.getByTestId("create-task-board").first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByTestId("task-title-input").fill("P12 created by spec");
    const priority = dialog.locator("#task-priority");
    await priority.click();
    // Phase 14 (WS3): each option is a two-line row — bold level name plus
    // a muted bilingual (fa — en) description — so match by the bold name.
    for (const level of ["None", "Low", "Medium", "High", "Urgent", "Critical"]) {
      await expect(
        page.getByRole("option").filter({ has: page.getByText(level, { exact: true }) }),
        `priority option ${level}`,
      ).toHaveCount(1);
    }
    await page
      .getByRole("option")
      .filter({ has: page.getByText("Urgent", { exact: true }) })
      .click();
    await dialog.getByTestId("task-submit").click();
    await expect(dialog).toBeHidden();
  });

  test("deadline chip renders on the list surface", async () => {
    await page.goto(`/projects/${projectId}?team=${teamId}&view=list`);
    // The flag icon (svg) renders inside the red-tinted chip.
    const deadlineChip = page.getByTestId("deadline-2027-01-01");
    await expect(deadlineChip).toBeVisible();
    await expect(deadlineChip.locator("svg")).toBeVisible();
  });

  test("checklist: add, toggle, combined progress bar", async () => {
    await page.goto(boardUrl());
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-detail");
    await expect(panel).toBeVisible();

    // Two items.
    await panel.getByTestId("checklist-input").fill("write spec");
    await panel.getByTestId("checklist-input").press("Enter");
    await panel.getByTestId("checklist-input").fill("review spec");
    await panel.getByTestId("checklist-input").press("Enter");
    await expect(panel.locator("li", { hasText: "write spec" })).toHaveCount(1);
    await expect(panel.locator("li", { hasText: "review spec" })).toHaveCount(1);

    // Toggle one — combined progress (no subtasks) shows 1/2 at 50%.
    const toggles = panel.locator("[role=checkbox]");
    await toggles.first().click();
    await expect(panel.getByTestId("checklist-progress")).toHaveText("50%");

    // Delete the second item; progress returns to 100%.
    await panel.locator("li", { hasText: "review spec" }).locator("button[aria-label^=Delete]").click();
    await expect(panel.getByTestId("checklist-progress")).toHaveText("100%");
    await page.keyboard.press("Escape");
  });

  test("attachments: upload, list, download, delete", async () => {
    await page.goto(boardUrl());
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-detail");
    await expect(panel.getByTestId("task-attachments")).toBeVisible();

    await panel.locator('input[type="file"]').setInputFiles({
      name: "notes-12.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("phase 12 attachment"),
    });
    await expect(panel.getByTestId("task-attachments").locator("li")).toHaveCount(1);
    await expect(panel.getByTestId("task-attachments").locator("li").first()).toContainText(
      "notes-12.txt",
    );

    // The download endpoint serves the bytes back.
    const href = await panel
      .getByTestId("task-attachments")
      .locator("a")
      .first()
      .getAttribute("href");
    expect(href).toBeTruthy();
    const download = await page.request.get(href!);
    expect(download.status()).toBe(200);
    expect(await download.text()).toBe("phase 12 attachment");

    // Delete clears the list.
    await panel.getByTestId("task-attachments").locator("button[aria-label^=Delete]").click();
    await expect(panel.getByTestId("task-attachments").locator("li")).toHaveCount(0);
    await page.keyboard.press("Escape");
  });

  test("closed tasks lock the detail panel", async () => {
    // Drive the legal lifecycle: the member (assignee) moves the task to
    // resolved; the admin (reporter) submits for review and accepts.
    const memberContext = await context.browser()!.newContext();
    const memberPage = await memberContext.newPage();
    await memberPage.goto("/login");
    await memberPage.getByLabel("Email").fill(MEMBER.email);
    await memberPage.getByLabel("Password").fill(MEMBER.password);
    await memberPage.getByTestId("login-submit").click();
    await expect(memberPage.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    for (const to of ["todo", "in_progress", "resolved"]) {
      const response = await memberPage.request.post(`/api/v1/tasks/${taskId}/transition`, {
        data: { to },
      });
      expect(response.status(), `member transition to ${to}`).toBe(200);
    }
    await memberContext.close();
    for (const to of ["in_review", "closed"]) {
      const response = await page.request.post(`/api/v1/tasks/${taskId}/transition`, {
        data: { to },
      });
      expect(response.status(), `admin transition to ${to}`).toBe(200);
    }
    await page.goto(boardUrl());
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-detail");
    await expect(panel.getByTestId("task-locked")).toContainText("Locked");
    await expect(panel.getByTestId("comments-locked")).toBeVisible();
    // Edit affordances are gone: no edit menu entry, no checklist composer.
    await expect(panel.getByTestId("task-actions")).toBeVisible();
    await page.getByTestId("task-actions").click();
    await expect(page.getByTestId("edit-task")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(panel.getByTestId("checklist-input")).toHaveCount(0);
    await expect(panel.getByTestId("attachment-upload")).toHaveCount(0);
  });

  test("theme switcher groups by appearance", async () => {
    await page.goto("/");
    await page.getByTestId("theme-switcher").click();
    await expect(page.getByText("Light", { exact: true }).first()).toBeVisible();
    const options = page.locator("[data-testid^=theme-option-]");
    await expect(options).toHaveCount(16);
    await page.keyboard.press("Escape");
  });

  test("detail sidebar toggles with Cmd+.", async () => {
    await page.goto(boardUrl());
    await page.getByTestId(`kanban-card-${taskId}`).click();
    await expect(page.getByTestId("task-panel")).toBeVisible();
    await page.keyboard.press("Control+.");
    await expect(page.getByTestId("task-panel-collapsed")).toBeVisible();
    await page.getByTestId("sidebar-expand").click();
    await expect(page.getByTestId("task-panel")).toBeVisible();
  });

  test("notifications dropdown has filter tabs", async () => {
    await page.goto("/");
    await page.getByTestId("notification-bell").click();
    await expect(page.getByTestId("notification-tab-all")).toBeVisible();
    await expect(page.getByTestId("notification-tab-unread")).toBeVisible();
    await page.keyboard.press("Escape");
  });

  test("dashboard shows KPI cards and window tabs", async () => {
    await page.goto(`/teams/${teamId}/dashboard`);
    await expect(page.getByTestId("dashboard-kpis")).toBeVisible();
    await expect(page.getByTestId("kpi-throughput")).toBeVisible();
    await expect(page.getByTestId("kpi-wip")).toBeVisible();
    await expect(page.getByTestId("kpi-capacity")).toBeVisible();
    await page.getByTestId("dashboard-window-30d").click();
    await expect(page.getByTestId("dashboard-kpis")).toBeVisible();

    // axe: no critical violations on the dashboard.
    const results = await new AxeBuilder({ page }).analyze();
    const critical = results.violations.filter((v) => (v.impact ?? "") === "critical");
    expect(critical, JSON.stringify(critical.map((v) => v.id))).toEqual([]);
  });

  test("label icon renders in the dialog picker", async () => {
    await page.goto(boardUrl());
    await page.getByTestId("create-task-board").first().click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByTestId("label-picker")).toBeVisible();
    // The seeded label carries the `bug` Lucide icon; its svg renders.
    await expect(dialog.locator('[data-testid=label-picker] svg').first()).toBeVisible();
    await page.keyboard.press("Escape");
  });
});
