import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 13 E2E: the eight fixes.
 *   1. "Open full page" affordance + ⌘⇧O (outside the overflow menu).
 *   2. Cross-team isolation (Team A task never appears in Team B).
 *   3. Full page hides the quick view; back restores it.
 *   4. Collapsible sidebar (⌘B, persisted).
 *   5. Users lives in Settings, not the sidebar; ⌘, opens Settings.
 *   6. List/Board/Timeline are separate routes; timeline renders.
 *   7. Guest role: invite → grant → restricted view → revoke → 404.
 *   8. Team avatar updates in real time.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.describe.serial("Phase 13 fixes", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let teamBId = "";
  let projectId = "";
  let taskId = "";
  const suffix = Date.now().toString(36);

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    page.on("response", (res) => {
      const u = res.url();
      if (u.includes("/auth/") || u.includes("/api/v1/me")) {
        console.log(
          "AUTH-RESP:",
          res.status(),
          res.request().method(),
          u.slice(21, 60),
          "set-cookie:",
          res.headers()["set-cookie"] !== undefined,
          "req-cookie:",
          res.request().headers()["cookie"] !== undefined,
        );
      }
    });
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    const team = await page.request.post("/api/v1/teams", {
      data: { name: `P13 Team A ${suffix}` },
    });
    teamId = (await team.json()).id as string;
    const teamB = await page.request.post("/api/v1/teams", {
      data: { name: `P13 Team B ${suffix}` },
    });
    teamBId = (await teamB.json()).id as string;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `P13 Project ${suffix}` },
    });
    projectId = (await project.json()).id as string;
    const task = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P13 seed task", deadline: "2027-06-01" },
    });
    taskId = ((await task.json()) as { id: string }).id;
    await page.request.post(`/api/v1/teams/${teamBId}/projects`, {
      data: { name: `P13 B Project ${suffix}` },
    });
  });

  test.afterAll(async () => {
    await context.close();
  });

  // -- Bug 2: cross-team isolation -----------------------------------------
  test("team A task never appears in team B views", async () => {
    await page.goto(`/projects/${projectId}?team=${teamId}&view=board`);
    await expect(page.getByTestId(`kanban-card-${taskId}`)).toBeVisible();

    // Switch to Team B via the switcher (Bug 3's project-path rule lands on
    // Team B's projects list, NOT Team A's project).
    await page.getByTestId("team-switcher").click();
    await page.getByRole("menu").getByText("P13 Team B", { exact: false }).click();
    await expect(page).toHaveURL(new RegExp(`team=${teamBId}`));
    await expect(page).not.toHaveURL(new RegExp(projectId));

    // Team B's board/list/timeline/dashboard show no Team A task.
    for (const view of ["list", "board", "timeline"] as const) {
      await page.goto(`/projects?team=${teamBId}&view=${view}`);
      const body = await page.locator("main").innerText();
      expect(body, `Team B ${view} view`).not.toContain("P13 seed task");
    }
    await page.goto(`/teams/${teamBId}/dashboard`);
    const dash = await page.locator("main").innerText();
    expect(dash).not.toContain("P13 seed task");

    // Direct URL to Team A's project under Team B's ?team= gets corrected.
    await page.goto(`/projects/${projectId}?team=${teamBId}`);
    await expect(page).toHaveURL(new RegExp(`team=${teamId}`));
  });

  // -- Bug 1: expand affordance + shortcut ---------------------------------
  test("open-full-page button sits in the header; ⌘⇧O works", async () => {
    await page.goto(`/projects/${projectId}?team=${teamId}&view=board`);
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-detail");
    await expect(panel).toBeVisible();

    // The expand button is visible WITHOUT the overflow menu.
    const expand = panel.getByTestId("expand-full-page");
    await expect(expand).toBeVisible();
    await expect(expand).toHaveAttribute("aria-label", "Open task as full page");

    // The full page is a lazy chunk; wait for the surface, then the URL.
    await expand.click();
    await expect(page.getByTestId("task-page")).toBeVisible();
    await expect(page).toHaveURL(
      new RegExp(`/teams/${teamId}/projects/${projectId}/tasks/${taskId}`),
    );

    // Back → panel again (Bug 3, asserted fully below).

    // Shortcut path: open the panel, press ⌘⇧O (Ctrl+Shift+O).
    await page.goBack();
    await expect(panel).toBeVisible();
    await page.keyboard.press("Control+Shift+O");
    await expect(page.getByTestId("task-page")).toBeVisible();
    console.log("MARK: shortcut path passed");
    await expect(page).toHaveURL(
      new RegExp(`/teams/${teamId}/projects/${projectId}/tasks/${taskId}`),
    );
    await page.goBack();
  });

  // -- Bug 3: panel hidden on the full page, restored on back ---------------
  test("full page hides the quick view; back restores it", async () => {
    await page.goto(`/projects/${projectId}?team=${teamId}&view=board`);
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-detail");
    await expect(panel).toBeVisible();

    await panel.getByTestId("expand-full-page").click();
    await expect(page.getByTestId("task-page")).toBeVisible();
    // The quick view is out of the layout: no task-panel aside.
    await expect(page.getByTestId("task-panel")).toHaveCount(0);
    // The page's own metadata rail is present.
    await expect(page.getByTestId("task-page-rail")).toBeVisible();

    // Back → the panel is restored.
    await page.goBack();
    await expect(page.getByTestId("task-panel")).toBeVisible();
    await expect(page.getByTestId("task-detail")).toBeVisible();
  });

  // -- Bug 4: collapsible sidebar -------------------------------------------
  test("sidebar toggles, persists, and ⌘B works", async () => {
    await page.goto(`/?team=${teamId}`);
    const sidebar = page.getByTestId("app-sidebar");
    const toggle = page.getByTestId("sidebar-toggle");
    await expect(sidebar).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");

    // Collapse → icon rail; reload → still collapsed.
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await page.reload();
    await expect(page.getByTestId("sidebar-toggle")).toHaveAttribute(
      "aria-expanded",
      "false",
    );

    // Expand via ⌘B.
    await page.keyboard.press("Control+b");
    await expect(page.getByTestId("sidebar-toggle")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await page.reload();
    await expect(page.getByTestId("sidebar-toggle")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });

  // -- Bug 5: Users in Settings, not the sidebar -----------------------------
  test("Users moved to Settings; ⌘, opens Settings", async () => {
    await page.goto(`/?team=${teamId}`);
    // Not in the sidebar.
    await expect(page.getByRole("link", { name: "Users" })).toHaveCount(0);

    // ⌘, opens Settings.
    await page.keyboard.press("Control+,");
    await expect(page).toHaveURL(/\/settings/);
    await expect(page.getByTestId("settings-page")).toBeVisible();
    await expect(page.getByText("Users", { exact: false }).first()).toBeVisible();

    // The account menu also carries Settings.
    await page.goto("/");
    await page.getByTestId("user-menu").click();
    await page.getByTestId("menu-settings").click();
    await expect(page).toHaveURL(/\/settings/);
  });

  // -- Bug 6: separate view routes -------------------------------------------
  test("list/board/timeline have their own URLs; switcher navigates", async () => {
    await page.goto(`/projects/${projectId}?team=${teamId}`);
    // Default: board.
    await expect(page).toHaveURL(new RegExp(`team=${teamId}`));

    for (const view of ["list", "timeline", "board"] as const) {
      await page.getByTestId(`view-${view}`).click();
      await expect(page).toHaveURL(new RegExp(`/${view}(\\?|$)`));
    }
    // Direct navigation + reload preserves the view.
    await page.goto(`/projects/${projectId}/timeline?team=${teamId}`);
    await expect(page.getByTestId("timeline-view")).toBeVisible();
    await expect(page.getByTestId("timeline-grid")).toBeVisible();
    // The redesigned timeline: today line + task rows + deadline diamond.
    await expect(page.getByTestId("timeline-today")).toBeVisible();
    await expect(page.getByTestId(`timeline-bar-${taskId}`)).toBeVisible();
    await expect(page.getByTestId(`timeline-row-${taskId}`)).toBeVisible();
    await page.reload();
    await expect(page.getByTestId("timeline-view")).toBeVisible();
  });

  // -- Bug 7: guest role full flow -------------------------------------------
  test("guest: invite, grant, restricted view, revoke", async () => {
    const guestEmail = `guest13-${suffix}@test.local`;
    const guestPw = "Guest-Pass-1234!";
    const guestTeamId = teamId;

    // Owner creates the account (Bug 2 model) and invites as guest.
    const created = await page.request.post("/api/v1/admin/users", {
      data: { email: guestEmail, display_name: "P13 Guest", temporary_password: guestPw },
    });
    expect(created.status()).toBe(201);

    await page.goto(`/teams/${guestTeamId}/settings`);
    await page.getByTestId("invite-guest").click();
    await page.getByTestId("guest-search-input").fill(guestEmail);
    await page.getByTestId("guest-eligible-list").getByRole("option").first().click();

    // Grant view_project + view_task via the settings permission editor.
    await page.locator("#grant-permission").selectOption("view_project");
    await page.locator("#grant-project").selectOption(projectId);
    await page.getByTestId("grant-permission-submit").first().click();

    // Guest logs in: the switcher lists the team; the project is visible.
    const guestContext = await context.browser()!.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(guestEmail);
    await gp.getByLabel("Password").fill(guestPw);
    await gp.getByTestId("login-submit").click();
    await expect(gp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await gp.goto(`/projects/${projectId}`);
    await expect(gp.getByTestId("main-content")).toContainText("P13 seed task");
    // Guest badge visible.
    await gp.goto(`/?team=${guestTeamId}`);
    await expect(gp.getByTestId("guest-badge")).toBeVisible();

    // Revoke via API (row delete) → immediate 404.
    const guests = await (await page.request.get(`/api/v1/teams/${guestTeamId}/guests2`)).json();
    const guestId = guests.items.find(
      (g: { user: { email: string } }) => g.user.email === guestEmail,
    ).guest_id as string;
    const revoked = await page.request.delete(
      `/api/v1/teams/${guestTeamId}/guests2/${guestId}`,
    );
    expect(revoked.status()).toBe(204);

    await gp.goto(`/projects/${projectId}`);
    await expect(gp.getByTestId("main-content")).not.toContainText("P13 seed task");
    await guestContext.close();
  });

  // -- Bug 8: team avatar real-time ------------------------------------------
  test("team avatar updates in the settings preview and switcher", async () => {
    await page.goto(`/teams/${teamId}/settings`);
    const png =
      "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAeUlEQVR4nO3PQQkAMAzAwCqpfymTNRF7HINABFzm7H7dcEEDWtCAFjSgBQ1oQQNa0IAWNKAFDWhBA1rQgBY0oAUNaEEDWtCAFjSgBQ1oQQNa0IAWNKAFDWhBA1rQgBY0oAUNaEEDWtCAFjSgBQ1oQQNa0IAWNKAFj13Kp0DxHeM4GQAAAABJRU5ErkJggg==";
    await page.locator("#team-avatar-upload").setInputFiles({
      name: "team.png",
      mimeType: "image/png",
      buffer: Buffer.from(png, "base64"),
    });
    // Settings preview carries a versioned team-avatar URL.
    await expect(
      page.locator('img[src*="/teams/"][src*="/avatar?v="]').first(),
    ).toBeVisible();
    // The switcher renders the same versioned URL.
    await page.goto(`/?team=${teamId}`);
    await expect(
      page.locator('[data-testid="team-switcher"] img[src*="/teams/"][src*="?v="]').first(),
    ).toBeVisible();
  });
});
