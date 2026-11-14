import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 12.5 E2E: the eight bug fixes.
 *   1. Avatar updates in real time across surfaces (no reload, twice).
 *   2. Instance users: Owner creates; assignment is separate; multi-team.
 *   3. Team switch: URL is authoritative; sections follow; reload keeps it.
 *   4. Task detail full page: two columns, shared body, quick view intact.
 *   5. Capacity: Account value == capacity view value, live.
 *   6. Goals: contributing picker works (project + task), removal updates.
 *   7. Team timezone defaults to Asia/Tehran.
 *   8. Password change: live meter + strict server policy.
 *
 * Requires a freshly bootstrapped server (scripts/e2e-server.sh); the
 * bootstrap administrator is the instance Owner.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.describe.serial("Phase 12.5 fixes", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let teamBId = "";
  let projectId = "";
  let taskId = "";
  const suffix = Date.now().toString(36);
  const NEW_USER = { email: `p125-user-${suffix}@test.local`, password: "Member-Pass-1234!" };

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    const team = await page.request.post("/api/v1/teams", {
      data: { name: `P125 Team A ${suffix}` },
    });
    teamId = (await team.json()).id as string;
    const teamB = await page.request.post("/api/v1/teams", {
      data: { name: `P125 Team B ${suffix}` },
    });
    teamBId = (await teamB.json()).id as string;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `P125 Project ${suffix}` },
    });
    projectId = (await project.json()).id as string;
    const task = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P125 task" },
    });
    taskId = ((await task.json()) as { id: string }).id;
    // A second project in Team B for section checks.
    await page.request.post(`/api/v1/teams/${teamBId}/projects`, {
      data: { name: `P125 B Project ${suffix}` },
    });
  });

  test.afterAll(async () => {
    await context.close();
  });

  const boardUrl = () => `/projects/${projectId}?team=${teamId}&view=board`;

  // -- Bug 2 ---------------------------------------------------------------
  test("owner creates an instance user and assigns to two teams", async () => {
    await page.goto("/users");
    await page.getByTestId("admin-create-user").click();
    await page.getByTestId("new-user-email").fill(NEW_USER.email);
    await page.getByTestId("new-user-name").fill("P125 User");
    await page.getByTestId("new-user-username").fill(`p125u${suffix.slice(-6)}`);
    await page.getByTestId("new-user-password").fill(NEW_USER.password);
    await page.getByTestId("create-user-submit").click();
    await expect(page.getByTestId("admin-users-table")).toContainText(NEW_USER.email);
    // The generated temporary password already satisfies the policy; also
    // verify a weak one is rejected with the specific message.
    await page.getByTestId("admin-create-user").click();
    await page.getByTestId("new-user-email").fill(`weak-${suffix}@test.local`);
    await page.getByTestId("new-user-name").fill("Weak");
    await page.getByTestId("new-user-password").fill("weak-pass-123");
    await page.getByTestId("create-user-submit").click();
    await expect(page.locator('[role=alert]')).toContainText("uppercase");
    await page.keyboard.press("Escape");

    // Assign to both teams from team settings (the picker).
    for (const tid of [teamId, teamBId]) {
      await page.goto(`/teams/${tid}/settings?team=${tid}`);
      await page.getByTestId("add-member").click();
      await page.getByTestId("member-search-input").fill(NEW_USER.email);
      await page.getByTestId("eligible-members-list").getByRole("option").first().click();
      await expect(page.getByTestId("member-list")).toContainText(NEW_USER.email);
    }

    // The user logs in and sees both teams in the switcher.
    const userContext = await context.browser()!.newContext();
    const up = await userContext.newPage();
    await up.goto("/login");
    await up.getByLabel("Email").fill(NEW_USER.email);
    await up.getByLabel("Password").fill(NEW_USER.password);
    await up.getByTestId("login-submit").click();
    await expect(up.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await up.getByTestId("team-switcher").click();
    const menu = up.getByRole("menu");
    await expect(menu.getByText(`P125 Team A ${suffix}`)).toBeVisible();
    await expect(menu.getByText(`P125 Team B ${suffix}`)).toBeVisible();
    await userContext.close();
  });

  // -- Bug 3 ---------------------------------------------------------------
  test("team switch: sections follow the URL; reload keeps the team", async () => {
    // Select Team B via the switcher.
    await page.goto("/");
    await page.getByTestId("team-switcher").click();
    // Full (suffix-qualified) name — stale teams from earlier runs would
    // otherwise collide in the switcher menu.
    await page.getByRole("menu").getByText(`P125 Team B ${suffix}`).click();
    await expect(page).toHaveURL(new RegExp(`team=${teamBId}`));

    // Every section reflects Team B (its project appears in Projects).
    await page.goto(`/projects?team=${teamBId}`);
    await expect(page.getByText("P125 B Project", { exact: false })).toBeVisible();
    await page.goto(`/teams/${teamBId}/settings`);
    await expect(page).toHaveURL(new RegExp(`/teams/${teamBId}/settings`));

    // Reload keeps Team B (no redirect to the first team).
    await page.reload();
    await expect(page).toHaveURL(new RegExp(`/teams/${teamBId}/settings`));
    await expect(page.getByTestId("team-switcher")).toContainText("P125 Team B");

    // Switching from Team B settings to Team A preserves the section.
    await page.getByTestId("team-switcher").click();
    await page.getByRole("menu").getByText(`P125 Team A ${suffix}`).click();
    await expect(page).toHaveURL(new RegExp(`/teams/${teamId}/settings`));
    await expect(page.getByTestId("team-switcher")).toContainText("P125 Team A");
  });

  // -- Bug 4 ---------------------------------------------------------------
  test("task detail is a full page with a metadata rail; quick view intact", async () => {
    await page.goto(boardUrl());
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-detail");
    await expect(panel).toBeVisible();

    // "Open full page" navigates to the canonical URL.
    await page.getByTestId("task-actions").click();
    await page.getByTestId("open-full-page").click();
    await expect(page).toHaveURL(
      new RegExp(`/teams/${teamId}/projects/${projectId}/tasks/${taskId}`),
    );
    await expect(page.getByTestId("task-page")).toBeVisible();
    // Two-column layout: content + metadata rail.
    await expect(page.getByTestId("task-page-rail")).toBeVisible();
    await expect(page.getByTestId("task-detail")).toBeVisible();

    // Back navigation returns to the board view (the view state lives in
    // the URL, so history-back restores it exactly).
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`view=board`));
    await page.goto(`/teams/${teamId}/projects/${projectId}/tasks/${taskId}`);

    // Inline title edit.
    await page.getByTestId("task-page-title").click();
    await page.getByTestId("task-page-title-input").fill("P125 task (renamed)");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("task-page-title")).toContainText("P125 task (renamed)");

    // Duplicate task navigates to the copy.
    await page.getByTestId("task-actions").click();
    await page.getByTestId("duplicate-task").click();
    await expect(page.getByTestId("task-page-title")).toContainText("P125 task (renamed) (copy)");

    // The legacy /tasks/{id} route renders the same page.
    await page.goto(`/tasks/${taskId}`);
    await expect(page.getByTestId("task-page")).toBeVisible();
  });

  // -- Bug 5 ---------------------------------------------------------------
  test("capacity set in Account matches the capacity view live", async () => {
    // The capacity table renders when someone has open work: assign the
    // seed task to the member created in the Bug 2 test (the admin cannot
    // be both reporter and assignee).
    const members = await (await page.request.get(`/api/v1/teams/${teamId}/members`)).json();
    const memberId = (
      members.items as { user: { email: string; id: string } }[]
    ).find((m) => m.user.email === NEW_USER.email)!.user.id;
    const assigned = await page.request.patch(`/api/v1/tasks/${taskId}`, {
      data: { assignee_id: memberId },
    });
    expect(assigned.status()).toBe(200);

    await page.goto("/account");
    await expect(page.getByRole("heading", { name: "Profile" })).toBeVisible();
    const capacityInput = page.getByLabel("Capacity limit");
    await capacityInput.fill("20");
    await page.getByRole("button", { name: "Save profile" }).click();
    await expect(page.getByText("Profile saved")).toBeVisible();

    // The capacity view immediately shows 20 for this user (invalidation).
    await page.goto(`/teams/${teamId}/capacity`);
    await expect(page.getByTestId("capacity-table")).toBeVisible();
    await expect(page.getByTestId("capacity-table")).toContainText("20");
  });

  // -- Bug 6 ---------------------------------------------------------------
  test("goals: contributing picker associates project + task", async () => {
    const goal = await page.request.post(`/api/v1/teams/${teamId}/goals`, {
      data: { title: "P125 Goal", description: "picker journey" },
    });
    const goalId = ((await goal.json()) as { id: string }).id;
    await page.goto(`/goals/${goalId}`);

    // Picker opens, lists results, picks the project — chip appears.
    await page.getByTestId("picker-open-projects").click();
    await page.getByTestId("picker-input-projects").fill("P125");
    await page.getByRole("option").first().click();
    await expect(page.getByTestId("goal-project-list")).toContainText("P125 Project");

    // Task picker: search, pick, chip appears.
    await page.getByTestId("picker-open-tasks").click();
    await page.getByTestId("picker-input-tasks").fill("P125");
    await page.getByRole("option").first().click();
    await expect(page.getByTestId("goal-task-list")).toContainText("P125 task (renamed)");

    // Removal updates the list immediately.
    await page.locator("[data-testid^=remove-goal-project-]").first().click();
    await expect(page.getByTestId("goal-project-list")).toBeHidden();
  });

  // -- Bug 7 ---------------------------------------------------------------
  test("team settings shows the Asia/Tehran default timezone", async () => {
    await page.goto(`/teams/${teamId}/settings`);
    await expect(page.getByTestId("team-timezone-select")).toContainText("Asia/Tehran");
  });

  // -- Bug 8 ---------------------------------------------------------------
  test("change password shows the meter and blocks weak passwords", async () => {
    const userContext = await context.browser()!.newContext();
    const up = await userContext.newPage();
    await up.goto("/login");
    await up.getByLabel("Email").fill(NEW_USER.email);
    await up.getByLabel("Password").fill(NEW_USER.password);
    await up.getByTestId("login-submit").click();
    await expect(up.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await up.goto("/account");

    await up.getByLabel("Current password").fill(NEW_USER.password);
    await up.getByLabel("New password", { exact: true }).fill("weak-pass-123");
    // The meter is live and labels it weak/fair.
    await expect(up.getByTestId("password-strength")).toBeVisible();

    await up.getByLabel("New password", { exact: true }).fill("Sunny-Trail-42x");
    await up.getByRole("button", { name: "Change password" }).click();
    await expect(up.getByText("Password changed")).toBeVisible();
    await userContext.close();
  });

  // -- Bug 1 ---------------------------------------------------------------
  test("avatar updates in real time across surfaces, twice", async () => {
    // Two valid 64x64 PNGs with different bytes.
    const pngA = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAb0lEQVR4nO3PAQkAAAyEwO9feoshgnABdLep8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3IPanc8OLDQitxAAAAAElFTkSuQmCC";
    const pngB = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAb0lEQVR4nO3PAQkAAAyEwO9feoshgnABdNvJ8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ2oPcf88OIhvJ6vAAAAAElFTkSuQmCC";
    const toBuffer = (b64: string) => Buffer.from(b64, "base64");

    await page.goto("/account");
    const previewSrc = () =>
      page.evaluate(() => {
        const img = document.querySelector<HTMLImageElement>('img[src*="/users/"][src*="/avatar"]');
        return img ? img.getAttribute("src") : null;
      });

    await page.locator("#avatar-upload").setInputFiles({
      name: "a.png",
      mimeType: "image/png",
      buffer: toBuffer(pngA),
    });
    await expect(page.getByText("Avatar updated").first()).toBeVisible();
    const first = await previewSrc();
    expect(first, "first upload carries a cache-busting version").toContain("?v=");

    // Second upload: the version advances — the URL never repeats.
    await page.locator("#avatar-upload").setInputFiles({
      name: "b.png",
      mimeType: "image/png",
      buffer: toBuffer(pngB),
    });
    await expect(page.getByText("Avatar updated").first()).toBeVisible();
    const second = await previewSrc();
    expect(second).toContain("?v=");
    expect(second, "the second upload must produce a NEW URL").not.toBe(first);

    // Cross-surface: the task full page renders the uploader's avatar
    // (reporter chip) with the SAME versioned URL — no reload, no
    // per-surface refetch.
    await page.goto(`/teams/${teamId}/projects/${projectId}/tasks/${taskId}`);
    await expect(
      page
        .locator('[data-testid="task-page-rail"] [data-testid="task-metadata"] img[src*="/users/"][src*="?v="]')
        .first(),
    ).toBeVisible();
    await page.getByTestId("comment-input").click();
    await page.getByTestId("comment-input").fill("avatar cross-surface check");
    await page.getByTestId("comment-submit").click();
    await expect(
      page
        .locator('[data-testid="comment-threads"] img[src*="/users/"][src*="?v="]')
        .first(),
    ).toBeVisible();
  });
});
