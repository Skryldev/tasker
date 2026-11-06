import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 6 E2E: the Persian (Solar Hijri) calendar is the user-facing calendar
 * while the API keeps receiving Gregorian ISO dates.
 *
 * Reference mapping used throughout (independent of the app under test):
 *   Aban 1, 1405  = 2026-10-23
 *   Aban 15, 1405 = 2026-11-06
 */

const ADMIN = { email: "admin@tasker.local", password: "admin-pass-12345" };

test.describe.serial("Phase 6 Persian calendar", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    await page.getByTestId("team-switcher").click();
    await page.getByText("Create team…").click();
    await page.getByLabel("Name").fill("P6 Team");
    await page.getByTestId("create-team-submit").click();
    await page.waitForURL(/\?team=/);
    teamId = new URL(await page.url()).searchParams.get("team") ?? "";

    await page.goto(`/projects?team=${teamId}`);
    await page.getByTestId("create-project").click();
    await page.getByTestId("project-name-input").fill("P6 Project");
    await page.getByTestId("project-submit").click();
    await page.getByRole("link", { name: /P6 Project/ }).first().click();
    projectId = new URL(await page.url()).pathname.split("/").pop() ?? "";
    expect(projectId).not.toBe("");
  });

  test.afterAll(async () => {
    await context.close();
  });

  test("task dialog shows the Persian calendar and submits the Gregorian day", async () => {
    await page.getByTestId("create-task").click();
    await page.getByTestId("task-title-input").fill("Calendar task");

    // Open the Doran calendar from the due-date field's icon button.
    const dueInput = page.getByLabel("Due date", { exact: true });
    await page.getByRole("button", { name: "Due date — تقویم" }).click();
    const grid = page.getByRole("grid");
    await expect(grid).toBeVisible();

    // Persian month header (the month containing 2026-11-01 is Aban 1405)
    // and Persian weekday initials in the grid.
    await expect(page.locator(".doran-calendar__header")).toContainText("آبان");
    await expect(grid).toContainText(/ش|ی|د|س/);

    // Pick 15 Aban 1405 → the trigger shows the Jalali value in Persian digits.
    await grid.locator("button", { hasText: /^۱۵$/ }).first().click();
    await expect(dueInput).toHaveValue("۱۴۰۵/۰۸/۱۵");

    await page.getByTestId("task-submit").click();
    await expect(page.getByTestId("task-table")).toContainText("Calendar task");

    // The API received the Gregorian ISO day — Aban 15, 1405 = 2026-11-06.
    const res = await page.request.get(
      `/api/v1/projects/${projectId}/tasks?q=Calendar%20task`,
    );
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.total).toBe(1);
    expect(body.items[0].due_date).toBe("2026-11-06");
  });

  test("workload date filters map the Jalali range to Gregorian query params", async () => {
    await page.goto(`/teams/${teamId}/workload`);
    await expect(page.getByRole("heading", { name: /Workload/i })).toBeVisible();

    // Type 1 Aban 1405 into the "due on or after" filter.
    const afterInput = page.getByLabel("Due on or after", { exact: true });
    await afterInput.fill("1405/08/01");
    await afterInput.blur();

    // The outgoing workload request carries the Gregorian day for Aban 1.
    const req = page.waitForRequest((r) => r.url().includes("/workload") && r.url().includes("due_after"));
    await page.reload();
    // After a reload the filter state is empty; type again to trigger a request.
    await expect(page.getByRole("heading", { name: /Workload/i })).toBeVisible();
    const afterInput2 = page.getByLabel("Due on or after", { exact: true });
    await afterInput2.fill("1405/08/01");
    await afterInput2.blur();
    const request = await req;
    expect(request.url()).toContain("due_after=2026-10-23");
  });

  test("read-only date displays render the Persian calendar", async () => {
    // The task created above has due_date 2026-11-06 = ۱۴۰۵/۰۸/۱۵.
    // Phase 10 §5.1: the list shows a relative due label; the absolute
    // Jalali date is the hover title, and the detail panel renders it inline.
    await page.goto(`/projects/${projectId}`);
    await expect(
      page.getByTestId("task-table").getByTitle("Due ۱۴۰۵/۰۸/۱۵"),
    ).toBeVisible();
    await page.getByTestId("task-table").getByRole("row").nth(1).click();
    await expect(page.getByTestId("task-detail")).toContainText("۱۴۰۵/۰۸/۱۵");
  });
});
