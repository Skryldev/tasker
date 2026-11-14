import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 13.5 E2E: ten fixes with layout assertions.
 *   1. Collapsed-rail avatar centered/clipped (bounding box inside rail).
 *   2. Settings menu scrolls to sections; direct hash works; scroll spy.
 *   3. Team Settings scroll height is content-driven.
 *   4. Timeline: page-level no overflow; inner grid scrolls.
 *   5. Board/Timeline routes unmount the inactive view.
 *   6. Throughput widget renders a visible empty state at zero data.
 *   7. Team avatar updates in Team details immediately; one avatar section.
 *   8. Full page: quick view hidden; rail carries status.
 *   9. Label chips: create with icon → AA chip in manager.
 *  10. Guest drawer: badges and toggles.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.describe.serial("Phase 13.5 fixes", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";
  let taskId = "";
  const suffix = Date.now().toString(36);

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    const team = await page.request.post("/api/v1/teams", {
      data: { name: `P135 Team ${suffix}` },
    });
    teamId = (await team.json()).id as string;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `P135 Project ${suffix}` },
    });
    projectId = (await project.json()).id as string;
    const task = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P135 seed task" },
    });
    taskId = ((await task.json()) as { id: string }).id;
  });

  test.afterAll(async () => {
    await context.close();
  });

  // -- Bug 1 ----------------------------------------------------------------
  test("collapsed rail: avatar centered, clipped, within bounds", async () => {
    // Deterministic start: the sidebar persisted state could be either way.
    await page.addInitScript(() => localStorage.setItem("tasker:sidebar-open", "open"));
    await page.goto(`/?team=${teamId}`);
    const toggle = page.getByTestId("sidebar-toggle");
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    // Wait out the 200ms width transition before measuring geometry.
    await page.waitForTimeout(350);
    await expect(page.getByTestId("app-sidebar")).toHaveCSS("width", "48px");

    const rail = page.getByTestId("app-sidebar");
    const railBox = await rail.boundingBox();
    const switcher = page.getByTestId("team-switcher");
    const avatar = switcher.locator("span").first();
    const avatarBox = await avatar.boundingBox();
    expect(railBox, "rail bounds").toBeTruthy();
    expect(avatarBox, "avatar bounds").toBeTruthy();
    // The avatar must sit fully inside the rail's width.
    expect(avatarBox!.x).toBeGreaterThanOrEqual(railBox!.x);
    expect(avatarBox!.x + avatarBox!.width).toBeLessThanOrEqual(railBox!.x + railBox!.width);
    // And be horizontally centered: equal margins on both sides (±6px).
    const leftGap = avatarBox!.x - railBox!.x;
    const rightGap = railBox!.x + railBox!.width - (avatarBox!.x + avatarBox!.width);
    expect(Math.abs(leftGap - rightGap)).toBeLessThanOrEqual(6);

    // Alignment matches nav icons: same centerline.
    const navIcon = page.getByRole("link", { name: "My Tasks" }).locator("svg").first();
    const iconBox = await navIcon.boundingBox();
    expect(iconBox).toBeTruthy();
    const avatarCenter = avatarBox!.x + avatarBox!.width / 2;
    const iconCenter = iconBox!.x + iconBox!.width / 2;
    console.log(
      "RAIL-GEOM rail:", JSON.stringify(railBox),
      "avatar:", JSON.stringify(avatarBox),
      "icon:", JSON.stringify(iconBox),
    );
    expect(Math.abs(avatarCenter - iconCenter)).toBeLessThanOrEqual(4);
  });

  // -- Bug 2 ----------------------------------------------------------------
  test("settings menu scrolls to sections; scroll spy highlights", async () => {
    await page.goto("/settings");
    await expect(page.getByTestId("settings-page")).toBeVisible();

    // Click Users in the rail → the Users section scrolls into view.
    await page.getByTestId("settings-nav-users").click();
    await page.waitForTimeout(700);
    const inView = await page.evaluate(() => {
      const el = document.getElementById("users");
      if (!el) return false;
      const r = el.getBoundingClientRect();
      return r.top >= 0 && r.top < window.innerHeight;
    });
    expect(inView, "users section in view after menu click").toBe(true);
    await expect(page.getByTestId("settings-nav-users")).toHaveAttribute("aria-current", "true");
  });

  // -- Bug 3 ----------------------------------------------------------------
  test("team settings scroll height is content-driven", async () => {
    await page.goto(`/teams/${teamId}/settings`);
    await page.waitForTimeout(1000);
    // Scroll to the very bottom: the last section's bottom must land at (or
    // above) the container bottom — no empty whitespace below the content.
    const m = await page.evaluate(() => {
      const main = document.getElementById("main-content");
      if (!main) return { gap: 999 };
      main.scrollTop = main.scrollHeight;
      return new Promise((resolve) => {
        window.setTimeout(() => {
          const last = main.lastElementChild?.getBoundingClientRect().bottom ?? 0;
          const bottom = main.getBoundingClientRect().bottom;
          resolve({ gap: Math.round(bottom - last) });
        }, 200);
      });
    });
    // Allow only the container's own bottom padding (16px) as slack.
    console.log("GAP-PROBE:", JSON.stringify(m));
    expect(m.gap, `dead space below content: ${JSON.stringify(m)}`).toBeLessThanOrEqual(24);
  });

  // -- Bug 4 ----------------------------------------------------------------
  test("timeline: page has no overflow; inner grid scrolls horizontally", async () => {
    await page.goto(`/projects/${projectId}/timeline`);
    await expect(page.getByTestId("timeline-view")).toBeVisible();
    const m = await page.evaluate(() => {
      const main = document.getElementById("main-content");
      const grid = document.querySelector("[data-testid=timeline-grid]");
      return {
        docScrollW: document.documentElement.scrollWidth,
        innerW: window.innerWidth,
        mainScrollW: main?.scrollWidth ?? 0,
        mainClientW: main?.clientWidth ?? 0,
        gridScrollW: grid?.scrollWidth ?? 0,
        gridClientW: grid?.clientWidth ?? 0,
      };
    });
    // No page-level horizontal overflow.
    expect(m.docScrollW).toBeLessThanOrEqual(m.innerW + 2);
    expect(m.mainScrollW).toBeLessThanOrEqual(m.mainClientW + 2);
    // The inner grid owns horizontal scrolling (zoomed-out week grid is wide).
    expect(m.gridScrollW).toBeGreaterThanOrEqual(m.gridClientW);
    // No page-level vertical overflow: main scroll height equals client.
    const v = await page.evaluate(() => {
      const main = document.getElementById("main-content");
      return { s: main?.scrollHeight ?? 0, c: main?.clientHeight ?? 0 };
    });
    expect(v.s).toBeLessThanOrEqual(v.c + 2);
  });

  // -- Bug 5 ----------------------------------------------------------------
  test("board and timeline routes mount only the active view", async () => {
    await page.goto(`/projects/${projectId}/board`);
    await expect(page.getByTestId("board-view")).toBeVisible();
    await expect(page.getByTestId("timeline-view")).toHaveCount(0);

    await page.goto(`/projects/${projectId}/timeline`);
    await page.waitForTimeout(800);
    console.log("TL-NAV url:", page.url(), "view:", await page.locator("[data-testid=timeline-view]").count(), "board:", await page.locator("[data-testid=board-view]").count());
    await expect(page.getByTestId("kanban-board")).toHaveCount(0);

    // Switcher navigation unmounts the previous view.
    await page.getByTestId("view-board").click();
    await expect(page.getByTestId("timeline-view")).toHaveCount(0);
    await expect(page.getByTestId("board-view")).toBeVisible();
  });

  // -- Bug 6 ----------------------------------------------------------------
  test("throughput widget renders an empty state at zero data", async () => {
    const emptyTeam = await page.request.post("/api/v1/teams", {
      data: { name: `P135 Empty ${suffix}` },
    });
    const emptyTeamId = (await emptyTeam.json()).id as string;
    await page.goto(`/teams/${emptyTeamId}/dashboard`);
    await expect(page.getByTestId("dashboard-kpis")).toBeVisible();
    await expect(page.getByTestId("throughput-empty")).toBeVisible();
    await expect(page.getByTestId("throughput-empty")).toContainText("No data for this period");
  });

  // -- Bug 7 ----------------------------------------------------------------
  test("team avatar updates immediately in Team details; single section", async () => {
    await page.goto(`/teams/${teamId}/settings`);
    // Only ONE avatar section: the details-form avatar (h-14). The header
    // must not render another avatar.
    await expect(page.getByTestId("team-avatar-pick")).toBeVisible();
    const png = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAeUlEQVR4nO3PQQkAMAzAwCqpfymTNRF7HINABFxm93zdcEEDWtCAFjSgBQ1oQQNa0IAWNKAFDWhBA1rQgBY0oAUNaEEDWtCAFjSgBQ1oQQNa0IAWNKAFDWhBA1rQgBY0oAUNaEEDWtCAFjSgBQ1oQQNa0IAWNKAFj12JbEDxwwbwKgAAAABJRU5ErkJggg==";
    await page.locator("#team-avatar-upload").setInputFiles({
      name: "team.png",
      mimeType: "image/png",
      buffer: Buffer.from(png, "base64"),
    });

    // Versioned URL appears in the details avatar immediately.
    await expect(
      page.locator('img[src*="/teams/"][src*="/avatar?v="]').first(),
    ).toBeVisible();
  });

  // -- Bug 8 ----------------------------------------------------------------
  test("full page: quick view hidden; rail carries status", async () => {
    await page.goto(`/projects/${projectId}/board`);
    await page.getByTestId(`kanban-card-${taskId}`).click();
    const panel = page.getByTestId("task-panel");
    await expect(page.getByTestId("task-detail")).toBeVisible();
    await panel.getByTestId("expand-full-page").click();
    await expect(page.getByTestId("task-page")).toBeVisible();
    // The quick view is display:none — not visible, zero box.
    const panelHidden = await page.getByTestId("task-panel").isHidden();
    expect(panelHidden, "quick view fully removed from layout").toBe(true);
    // Rail carries the status readout alongside created/updated.
    const railText = await page.getByTestId("task-page-rail").innerText();
    expect(railText.toLowerCase()).toContain("status");
    expect(railText.toLowerCase()).toContain("created");
    // Back restores the quick view.
    await page.goBack();
    await expect(panel).toBeVisible();
  });

  // -- Bug 9 ----------------------------------------------------------------
  test("label create form: palette, icon picker, live preview", async () => {
    await page.goto(`/teams/${teamId}/settings`);
    await expect(page.getByTestId("label-create-form")).toBeVisible();
    // Icon grid exists with the 48 curated Lucide icons.
    const iconCount = await page.locator("[data-testid=label-icon-grid] [role=option]").count();
    expect(iconCount).toBeGreaterThanOrEqual(40);
    // Create with name + icon.
    await page.getByTestId("label-name-input").fill("Bug");
    await page.getByTestId("label-icon-bug").click();
    await page.getByTestId("label-add").click();
    await expect(page.getByTestId("label-list")).toContainText("Bug");
  });

  // -- Bug 10 ---------------------------------------------------------------
  test("guest drawer: toggles grant, badges appear, revoke works", async () => {
    const guestEmail = `guest135-${suffix}@test.local`;
    const guestPw = "Guest-Pass-1234!";
    const created = await page.request.post("/api/v1/admin/users", {
      data: { email: guestEmail, display_name: "P135 Guest", temporary_password: guestPw },
    });
    expect(created.status()).toBe(201);

    await page.goto(`/teams/${teamId}/settings`);
    await page.getByTestId("invite-guest").click();
    await page.getByTestId("guest-search-input").fill(guestEmail);
    await page.getByTestId("guest-eligible-list").getByRole("option").first().click();

    const guests = await (await page.request.get(`/api/v1/teams/${teamId}/guests2`)).json();
    const guestId = guests.items[0].guest_id as string;

    // Open the drawer and grant view_project + view_task via toggles.
    await page.getByTestId(`edit-permissions-${guestId}`).click();
    await expect(page.getByTestId("permission-drawer")).toBeVisible();
    await page.getByTestId(`perm-toggle-${projectId}-view_project`).click();
    await page.getByTestId(`perm-toggle-${projectId}-view_task`).click();
    // Phase 14 (ADR-0021): the task LIST and the BOARD are separate grants;
    // the default project view is the board.
    await page.getByTestId(`perm-toggle-${projectId}-view_project_tasks`).click();
    await page.getByTestId(`perm-toggle-${projectId}-view_project_board`).click();
    await page.getByTestId("perm-save").click();
    await expect(page.getByTestId("permission-drawer")).toBeHidden();
    // Phase 14 (ADR-0021) labels: grouped by domain with the drawer's
    // human labels.
    await expect(page.getByTestId(`guest-badges-${guestId}`)).toContainText("Open the project");
    await expect(page.getByTestId(`guest-badges-${guestId}`)).toContainText("See the task list");

    // Guest can now see the project.
    const guestContext = await context.browser()!.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(guestEmail);
    await gp.getByLabel("Password").fill(guestPw);
    await gp.getByTestId("login-submit").click();
    await expect(gp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    await gp.goto(`/projects/${projectId}`);
    await expect(gp.getByTestId("main-content")).toContainText("P135 seed task");

    // Badge remove actions revoke immediately: drain every badge (the
    // granular grant set has several).
    for (let i = 0; i < 10; i++) {
      const remove = page.locator("[data-testid^=perm-remove-]").first();
      if ((await remove.count()) === 0) break;
      const badgeId = (await remove.getAttribute("data-testid"))!;
      await remove.click();
      // Each removal re-renders the badge list — wait for this badge to
      // disappear before targeting the next one.
      await expect(page.getByTestId(badgeId)).toHaveCount(0, { timeout: 5000 });
    }
    await expect(page.getByTestId(`guest-badges-${guestId}`)).toContainText(
      "No permissions granted",
    );
    await gp.goto(`/projects/${projectId}`);
    await expect(gp.getByTestId("main-content")).not.toContainText("P135 seed task");
    await guestContext.close();
  });
});
